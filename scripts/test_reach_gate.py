import contextlib
import io
import json
import re
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import reach_gate
import reach_truth
import scip_decode

PREFLIGHT = Path(__file__).with_name("release-preflight.sh")


def site(path, start, end=None, label=None):
    row = {
        "from_symbol_name": "caller",
        "from_symbol_id": "id",
        "to_symbol_name": "target",
        "kind": "calls",
        "path": path,
        "start_line": start,
        "start_column": 0,
    }
    if end is not None:
        row["end_line"] = end
    if label is not None:
        row["label"] = label
        row["basis"] = "name only"
    return row


def report_answer(sites, text_only=()):
    return reach_gate.parse_answer({"target": {}, "counts": {}, "sites": sites, "text_only": list(text_only)})


def result(language, split, truth, answer, repo="demo", name="n"):
    return {
        "repo": repo,
        "language": language,
        "split": split,
        "id": f"{name}-{split}",
        "name": name,
        "file": "a.py",
        "line": 1,
        "tags": [],
        "status": "ok",
        "detail": "",
        "counts": reach_gate.score_symbol(truth, answer),
    }


def varint(value):
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def length_delimited(number, payload):
    return varint(number << 3 | 2) + varint(len(payload)) + payload


def varint_field(number, value):
    return varint(number << 3) + varint(value)


def packed(values):
    return b"".join(varint(value) for value in values)


def occurrence_bytes(line, start, end, symbol, roles=0, enclosing=None):
    body = length_delimited(1, packed([line - 1, start, end])) + length_delimited(2, symbol.encode())
    if roles:
        body += varint_field(3, roles)
    if enclosing:
        body += length_delimited(7, packed([enclosing[0] - 1, 0, enclosing[1] - 1, 0]))
    return body


def info_bytes(symbol, implements=()):
    body = length_delimited(1, symbol.encode())
    for target in implements:
        body += length_delimited(4, length_delimited(1, target.encode()) + varint_field(3, 1))
    return body


def document_bytes(path, occurrences, infos=()):
    body = length_delimited(1, path.encode())
    for occurrence in occurrences:
        body += length_delimited(2, occurrence)
    for info in infos:
        body += length_delimited(3, info)
    return body


def index_bytes(documents):
    return b"".join(length_delimited(2, document) for document in documents)


def lines(path, count):
    return {(path, number) for number in range(1, count + 1)}


class SpanMatchingTest(unittest.TestCase):
    def test_site_covers_every_line_of_its_span(self):
        row = site("a.py", 10, end=12)
        for line in (10, 11, 12):
            self.assertTrue(reach_gate.site_covers(row, "a.py", line))
        for line in (9, 13):
            self.assertFalse(reach_gate.site_covers(row, "a.py", line))
        self.assertFalse(reach_gate.site_covers(row, "b.py", 11))

    def test_site_without_end_line_covers_only_its_start_line(self):
        row = site("a.py", 10)
        self.assertTrue(reach_gate.site_covers(row, "a.py", 10))
        self.assertFalse(reach_gate.site_covers(row, "a.py", 11))

    def test_site_without_start_line_covers_nothing(self):
        self.assertFalse(reach_gate.site_covers(site("a.py", None), "a.py", 1))


class AdjudicationTest(unittest.TestCase):
    def test_false_verdict_removes_a_line_and_true_verdict_adds_one(self):
        symbol = {
            "truth": [{"path": "a.py", "line": 1}, {"path": "a.py", "line": 2}],
            "adjudicated": [
                {"path": "a.py", "line": 2, "verdict": "false", "note": "a comment"},
                {"path": "a.py", "line": 3, "verdict": "true", "note": "a call"},
            ],
        }
        self.assertEqual(reach_gate.effective_truth(symbol), {("a.py", 1), ("a.py", 3)})

    def test_symbol_without_adjudications_keeps_its_truth(self):
        symbol = {"truth": [{"path": "a.py", "line": 1}]}
        self.assertEqual(reach_gate.effective_truth(symbol), {("a.py", 1)})

    def test_unknown_verdict_is_an_error(self):
        symbol = {"truth": [], "adjudicated": [{"path": "a.py", "line": 1, "verdict": "maybe", "note": ""}]}
        with self.assertRaises(ValueError):
            reach_gate.effective_truth(symbol)


class AnswerShapeTest(unittest.TestCase):
    def test_legacy_array_scores_every_row_as_proven_without_text_lines(self):
        answer = reach_gate.parse_answer([site("a.py", 1), site("a.py", 5)])
        self.assertEqual(answer["shape"], "legacy")
        self.assertEqual([row["label"] for row in answer["sites"]], ["proven", "proven"])
        self.assertEqual(answer["text_only"], set())
        counts = reach_gate.score_symbol({("a.py", 1)}, answer)
        self.assertEqual((counts["sites_proven"], counts["true_proven"]), (2, 1))
        self.assertEqual((counts["recalled_lines"], counts["accounted_lines"]), (1, 1))

    def test_report_object_keeps_labels_and_text_only_lines(self):
        answer = report_answer(
            [site("a.py", 1, label="likely")], text_only=[{"path": "d.rst", "lines": [4, 9]}]
        )
        self.assertEqual(answer["shape"], "report")
        self.assertEqual(answer["sites"][0]["label"], "likely")
        self.assertEqual(answer["text_only"], {("d.rst", 4), ("d.rst", 9)})

    def test_unknown_label_is_an_error(self):
        with self.assertRaises(ValueError):
            report_answer([site("a.py", 1, label="maybe")])

    def test_output_that_is_neither_array_nor_report_is_an_error(self):
        with self.assertRaises(ValueError):
            reach_gate.parse_answer("not json shaped")

    def test_scope_keeps_only_sites_and_text_lines_under_its_prefixes(self):
        answer = report_answer(
            [site("Src/Lib/a.cs", 1, label="proven"), site("Src/Lib.Tests/b.cs", 2, label="proven")],
            text_only=[{"path": "Src/Lib/c.cs", "lines": [3]}, {"path": "Doc/d.md", "lines": [4]}],
        )
        scoped = reach_gate.within_scope(answer, ["Src/Lib/"])
        self.assertEqual([row["path"] for row in scoped["sites"]], ["Src/Lib/a.cs"])
        self.assertEqual(scoped["text_only"], {("Src/Lib/c.cs", 3)})
        self.assertIs(reach_gate.within_scope(answer, None), answer)


class TierMetricsTest(unittest.TestCase):
    def test_label_tiers_count_sites_and_true_sites_apart(self):
        truth = lines("a.py", 4)
        answer = report_answer(
            [
                site("a.py", 1, label="proven"),
                site("a.py", 50, label="proven"),
                site("a.py", 2, label="likely"),
                site("a.py", 3, label="unresolved"),
                site("a.py", 4, end=5, label="excluded"),
                site("a.py", 9, label="excluded"),
            ]
        )
        counts = reach_gate.score_symbol(truth, answer)
        self.assertEqual((counts["sites_proven"], counts["true_proven"]), (2, 1))
        self.assertEqual((counts["sites_likely"], counts["true_likely"]), (1, 1))
        self.assertEqual((counts["sites_unresolved"], counts["true_unresolved"]), (1, 1))
        self.assertEqual((counts["sites_excluded"], counts["true_excluded"]), (2, 1))
        self.assertEqual(counts["truth_lines"], 4)
        self.assertEqual(counts["recalled_lines"], 2)
        self.assertEqual(counts["accounted_lines"], 4)
        metrics = reach_gate.aggregate([counts])
        self.assertEqual(metrics["proven_precision"], 0.5)
        self.assertEqual(metrics["recall"], 0.5)
        self.assertEqual(metrics["accounted"], 1.0)

    def test_text_only_lines_count_as_accounted_but_not_recalled(self):
        truth = {("a.py", 1), ("d.rst", 4), ("d.rst", 5)}
        answer = report_answer(
            [site("a.py", 1, label="proven")], text_only=[{"path": "d.rst", "lines": [4]}]
        )
        counts = reach_gate.score_symbol(truth, answer)
        self.assertEqual(counts["accounted_lines"], 2)
        self.assertEqual(counts["recalled_lines"], 1)
        self.assertAlmostEqual(reach_gate.aggregate([counts])["accounted"], 2 / 3)

    def test_precision_is_undefined_without_proven_sites(self):
        counts = reach_gate.score_symbol({("a.py", 1)}, reach_gate.parse_answer([]))
        metrics = reach_gate.aggregate([counts])
        self.assertIsNone(metrics["proven_precision"])
        self.assertEqual(metrics["recall"], 0.0)

    def test_aggregate_sums_counts_over_symbols(self):
        first = reach_gate.score_symbol({("a.py", 1)}, reach_gate.parse_answer([site("a.py", 1)]))
        second = reach_gate.score_symbol({("a.py", 2)}, reach_gate.parse_answer([site("a.py", 7)]))
        metrics = reach_gate.aggregate([first, second])
        self.assertEqual(metrics["symbols"], 2)
        self.assertEqual(metrics["proven_precision"], 0.5)
        self.assertEqual(metrics["recall"], 0.5)


class DecisionTest(unittest.TestCase):
    def passing(self, language="python", split="dev", repo="demo"):
        truth = lines("a.py", 100)
        answer = reach_gate.parse_answer([site("a.py", n) for n in range(1, 101)])
        return result(language, split, truth, answer, repo=repo)

    def test_gate_passes_when_every_language_meets_both_marks(self):
        report = reach_gate.build_report([self.passing()], ["demo"], ["demo"])
        self.assertEqual(report["failures"], [])
        self.assertEqual(report["result"], "PASS")

    def test_gate_fails_on_low_precision_in_one_language(self):
        truth = lines("a.py", 100)
        wrong = [site("a.py", n) for n in range(1, 101)] + [site("a.py", n) for n in (200, 201, 202)]
        weak = result("go", "dev", truth, reach_gate.parse_answer(wrong), repo="cobra")
        report = reach_gate.build_report([self.passing(), weak], ["demo", "cobra"], ["demo", "cobra"])
        self.assertEqual(report["result"], "FAIL")
        self.assertEqual(len(report["failures"]), 1)
        self.assertIn("go: proven precision 0.971", report["failures"][0])

    def test_gate_fails_on_low_accounted_share(self):
        truth = lines("a.py", 100)
        answer = report_answer([site("a.py", n, label="proven") for n in range(1, 91)])
        report = reach_gate.build_report([result("python", "dev", truth, answer)], ["demo"], ["demo"])
        self.assertEqual(report["result"], "FAIL")
        self.assertIn("python: accounted share 0.900", report["failures"][0])

    def test_text_only_lines_lift_the_accounted_share_to_the_mark(self):
        truth = lines("a.py", 100)
        answer = report_answer(
            [site("a.py", n, label="proven") for n in range(1, 99)],
            text_only=[{"path": "a.py", "lines": [99, 100]}],
        )
        report = reach_gate.build_report([result("python", "dev", truth, answer)], ["demo"], ["demo"])
        self.assertEqual(report["result"], "PASS")

    def test_gate_fails_when_a_corpus_repo_is_not_scored(self):
        report = reach_gate.build_report([self.passing()], ["demo"], ["demo", "flask", "zod"])
        self.assertEqual(report["result"], "FAIL")
        self.assertIn("not scored: flask, zod", report["failures"])

    def test_gate_fails_without_proven_sites(self):
        truth = lines("a.py", 10)
        answer = report_answer([site("a.py", n, label="likely") for n in range(1, 11)])
        report = reach_gate.build_report([result("python", "dev", truth, answer)], ["demo"], ["demo"])
        self.assertEqual(report["result"], "FAIL")
        self.assertIn("python: proven precision has no proven sites", report["failures"])

    def test_gate_fails_when_nothing_was_scored(self):
        report = reach_gate.build_report([], [], ["demo"])
        self.assertEqual(report["result"], "FAIL")

    def with_exclusions(self, split, wrong, clean, language="typescript", repo="zod"):
        truth = lines("a.py", 100)
        sites = [site("a.py", n, label="proven") for n in range(1, 101)]
        sites += [site("a.py", n, label="excluded") for n in range(1, wrong + 1)]
        sites += [site("b.py", n, label="excluded") for n in range(1, clean + 1)]
        return result(language, split, truth, report_answer(sites), repo=repo)

    def test_gate_fails_when_wrong_exclusions_drop_a_language_below_the_mark(self):
        rows = [self.with_exclusions("dev", wrong=0, clean=10), self.with_exclusions("holdout", wrong=1, clean=9)]
        report = reach_gate.build_report(rows, ["zod"], ["zod"])
        tiers = report["languages"]["typescript"]
        self.assertEqual(tiers["dev"]["excluded_precision"], 1.0)
        self.assertEqual(tiers["holdout"]["excluded_precision"], 0.9)
        self.assertEqual(tiers["all"]["excluded_precision"], 0.95)
        self.assertEqual(report["result"], "FAIL")
        self.assertEqual(report["failures"], ["typescript: excluded precision 0.950 is below 0.98"])

    def test_excluded_precision_at_the_mark_passes(self):
        report = reach_gate.build_report([self.with_exclusions("dev", wrong=1, clean=49)], ["zod"], ["zod"])
        self.assertEqual(report["languages"]["typescript"]["all"]["excluded_precision"], 0.98)
        self.assertEqual(report["result"], "PASS")

    def test_language_without_excluded_sites_passes_and_shows_no_data(self):
        report = reach_gate.build_report([self.passing()], ["demo"], ["demo"])
        self.assertIsNone(report["languages"]["python"]["all"]["excluded_precision"])
        self.assertEqual(report["result"], "PASS")
        text = reach_gate.render_markdown(report)
        self.assertIn("| Proven precision | Excluded precision | Recall | Accounted |", text)
        self.assertIn("| python | all | 1 | 100 | 100 | 1.000 | n/a | 1.000 | 1.000 |", text)
        self.assertIn("excluded precision >= 0.98", text)

    def test_marks_apply_to_each_language_not_to_the_overall_mix(self):
        truth = lines("a.py", 100)
        poor = reach_gate.parse_answer([site("a.py", n) for n in range(1, 51)] + [site("b.py", n) for n in range(1, 51)])
        weak = result("rust", "dev", truth, poor, repo="code-kb")
        report = reach_gate.build_report([self.passing(), weak], ["demo", "code-kb"], ["demo", "code-kb"])
        self.assertEqual(report["result"], "FAIL")
        self.assertTrue(all(failure.startswith("rust:") for failure in report["failures"]))


class HoldoutHidingTest(unittest.TestCase):
    def setUp(self):
        passing = DecisionTest().passing
        self.results = [passing(split="dev"), passing(split="holdout")]

    def test_holdout_symbols_stay_out_of_the_report_unless_shown(self):
        hidden = reach_gate.build_report(self.results, ["demo"], ["demo"])
        self.assertEqual({row["split"] for row in hidden["symbols"]}, {"dev"})
        self.assertEqual(hidden["languages"]["python"]["holdout"]["symbols"], 1)
        shown = reach_gate.build_report(self.results, ["demo"], ["demo"], show_holdout=True)
        self.assertEqual({row["split"] for row in shown["symbols"]}, {"dev", "holdout"})

    def test_markdown_omits_holdout_symbol_rows_but_keeps_the_aggregate(self):
        hidden = reach_gate.render_markdown(reach_gate.build_report(self.results, ["demo"], ["demo"]))
        self.assertNotIn("n-holdout", hidden)
        self.assertIn("n-dev", hidden)
        self.assertRegex(hidden, r"\| python \| holdout \| 1 \|")

    def test_markdown_ends_with_the_result_line(self):
        report = reach_gate.build_report(self.results, ["demo"], ["demo"])
        report["commit"] = "abc123"
        text = reach_gate.render_markdown(report)
        self.assertEqual(text.rstrip("\n").splitlines()[-1], "## Result: PASS")
        self.assertIn("\nCommit: abc123\n", text)
        failing = reach_gate.build_report(self.results, ["demo"], ["demo", "flask"])
        self.assertEqual(reach_gate.render_markdown(failing).rstrip("\n").splitlines()[-1], "## Result: FAIL")


class DisagreementTest(unittest.TestCase):
    def test_lists_proven_sites_without_truth_and_truth_lines_without_sites(self):
        truth = {("a.py", 1), ("a.py", 2), ("a.py", 5), ("d.rst", 4)}
        answer = report_answer(
            [
                site("a.py", 1, label="proven"),
                site("a.py", 5, label="excluded"),
                site("a.py", 30, label="proven"),
                site("a.py", 31, label="likely"),
                site("a.py", 32, label="excluded"),
            ],
            text_only=[{"path": "d.rst", "lines": [4]}],
        )
        found = reach_gate.disagreements(truth, answer)
        self.assertEqual([row["start_line"] for row in found["proven_without_truth"]], [30])
        self.assertEqual([row["start_line"] for row in found["excluded_with_truth"]], [5])
        self.assertEqual(found["truth_without_site"], [("a.py", 2, False), ("d.rst", 4, True)])


LOOKUP_ROWS = [
    {"symbol_id": "far", "path": "f.py", "name": "go", "kind": "function", "start_line": 40},
    {"symbol_id": "near", "path": "f.py", "name": "go", "kind": "function", "start_line": 10},
    {"symbol_id": "import", "path": "f.py", "name": "go", "kind": "import", "start_line": 11},
    {"symbol_id": "other-file", "path": "g.py", "name": "go", "kind": "function", "start_line": 11},
    {"symbol_id": "other-name", "path": "f.py", "name": "goes", "kind": "function", "start_line": 11},
]


class PickSymbolIdTest(unittest.TestCase):
    def symbol(self, line):
        return {"name": "go", "file": "f.py", "line": line, "kind": "function"}

    def test_exact_line_wins_and_otherwise_the_nearest_row_is_taken(self):
        self.assertEqual(reach_gate.pick_symbol_id(LOOKUP_ROWS, self.symbol(40)), "far")
        self.assertEqual(reach_gate.pick_symbol_id(LOOKUP_ROWS, self.symbol(10)), "near")
        self.assertEqual(reach_gate.pick_symbol_id(LOOKUP_ROWS, self.symbol(33)), "far")

    def test_full_text_rows_are_unwrapped_and_a_missing_symbol_gives_none(self):
        wrapped = [{"symbol": row, "score": 1.0} for row in LOOKUP_ROWS]
        self.assertEqual(reach_gate.pick_symbol_id(wrapped, self.symbol(10)), "near")
        self.assertIsNone(reach_gate.pick_symbol_id(LOOKUP_ROWS, {"name": "nope", "file": "f.py", "line": 1}))

    def test_a_lookup_object_offers_its_name_rows_and_ranked_rows(self):
        names_only = {"names": LOOKUP_ROWS[2:], "ranked": []}
        ranked = {"names": [], "ranked": [{"symbol": row, "score": 1.0} for row in LOOKUP_ROWS]}
        self.assertIsNone(reach_gate.pick_symbol_id(names_only, self.symbol(10)))
        self.assertEqual(reach_gate.pick_symbol_id(ranked, self.symbol(10)), "near")


STUB = """#!{python}
import json
import sys

args = sys.argv[1:]
if "--version" in args:
    print("code-kb 0.0.0")
    sys.exit(0)
command = next(arg for arg in args if arg in ("scan", "lookup", "refs"))
if command == "scan":
    sys.exit(0)
if command == "lookup":
    print(json.dumps([{{"symbol_id": "id1", "path": "f.py", "name": "go", "kind": "function", "start_line": 3}}]))
else:
    row = {{"from_symbol_name": "c", "from_symbol_id": "x", "to_symbol_name": "go", "kind": "calls", "start_column": 0}}
    print(json.dumps({{
        "target": {{}},
        "counts": {{}},
        "sites": [
            dict(row, path="f.py", start_line=10, label="proven", basis="same file"),
            dict(row, path="f.py", start_line=11, end_line=12, label="likely", basis="name only"),
        ],
        "text_only": [{{"path": "d.rst", "lines": [4]}}],
    }}))
"""


def gate_fixture(root):
    corpus = root / "corpus.json"
    entry = {"name": "demo", "language": "python", "url": "unused", "commit": "a" * 40, "local": None}
    corpus.write_text(json.dumps([entry]))
    cache = root / "cache"
    (cache / "demo-aaaaaaaaaaaa").mkdir(parents=True)
    (cache / "demo-aaaaaaaaaaaa" / "f.py").write_text("\n".join(f"line {n}" for n in range(1, 21)) + "\n")
    cases = root / "cases"
    cases.mkdir()
    truth = [{"path": "f.py", "line": 10}, {"path": "f.py", "line": 12}, {"path": "d.rst", "line": 4}]
    symbols = [
        {"id": f"go-{split}", "split": split, "name": "go", "file": "f.py", "line": 3, "kind": "function",
         "tags": [], "truth_source": "scip", "truth": truth, "adjudicated": []}
        for split in ("dev", "holdout")
    ]
    (cases / "demo.json").write_text(json.dumps({"repo": "demo", "commit": "a" * 40, "language": "python", "symbols": symbols}))
    stub = root / "stub-code-kb"
    stub.write_text(STUB.format(python=sys.executable))
    stub.chmod(0o755)
    return stub


def gate_args(command, root, stub, extra=()):
    return [
        command, "--binary", str(stub), "--corpus", str(root / "corpus.json"), "--cache-dir", str(root / "cache"),
        "--cases-dir", str(root / "cases"), *extra,
    ]


def quiet_main(argv):
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        return reach_gate.main(argv)


class RunTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.stub = gate_fixture(self.root)

    def test_run_scores_cases_with_a_stub_binary_and_writes_both_reports(self):
        out = self.root / "out"
        code = quiet_main(gate_args("run", self.root, self.stub, ["--repos", "demo", "--out", str(out)]))
        self.assertEqual(code, 0)
        report = json.loads((out / "report.json").read_text())
        self.assertEqual(report["result"], "PASS")
        self.assertEqual(report["languages"]["python"]["all"]["accounted"], 1.0)
        self.assertEqual(report["languages"]["python"]["all"]["proven_precision"], 1.0)
        text = (out / "report.md").read_text()
        self.assertEqual(text.rstrip("\n").splitlines()[-1], "## Result: PASS")
        self.assertRegex(text, r"(?m)^Commit: [0-9a-f]{7,40}$")
        self.assertNotIn("go-holdout", text)

    def test_run_exits_one_and_still_writes_the_report_when_a_mark_is_missed(self):
        out = self.root / "out"
        cases = self.root / "cases" / "demo.json"
        case = json.loads(cases.read_text())
        for symbol in case["symbols"]:
            symbol["truth"].append({"path": "f.py", "line": 18})
        cases.write_text(json.dumps(case))
        code = quiet_main(gate_args("run", self.root, self.stub, ["--out", str(out)]))
        self.assertEqual(code, 1)
        self.assertEqual((out / "report.md").read_text().rstrip("\n").splitlines()[-1], "## Result: FAIL")

    def test_run_without_a_case_file_reports_the_repo_as_not_scored(self):
        (self.root / "cases" / "demo.json").unlink()
        out = self.root / "out"
        code = quiet_main(gate_args("run", self.root, self.stub, ["--out", str(out)]))
        self.assertEqual(code, 1)
        self.assertIn("not scored: demo", json.loads((out / "report.json").read_text())["failures"])

    def test_disagreements_show_dev_detail_and_only_aggregates_for_holdout(self):
        capture = io.StringIO()
        with contextlib.redirect_stdout(capture):
            code = reach_gate.main(gate_args("disagreements", self.root, self.stub, ["--repo", "demo", "--split", "all"]))
        self.assertEqual(code, 0)
        text = capture.getvalue()
        self.assertIn("go-dev", text)
        self.assertNotIn("go-holdout", text)
        self.assertRegex(text, r"holdout: 1 symbols")
        shown = io.StringIO()
        with contextlib.redirect_stdout(shown):
            reach_gate.main(gate_args("disagreements", self.root, self.stub, ["--repo", "demo", "--split", "all", "--show-holdout"]))
        self.assertIn("go-holdout", shown.getvalue())

    def test_disagreements_name_a_proven_site_with_no_truth_line_and_its_source_text(self):
        cases = self.root / "cases" / "demo.json"
        case = json.loads(cases.read_text())
        for symbol in case["symbols"]:
            symbol["truth"] = [{"path": "f.py", "line": 12}, {"path": "f.py", "line": 15}]
        cases.write_text(json.dumps(case))
        capture = io.StringIO()
        with contextlib.redirect_stdout(capture):
            reach_gate.main(gate_args("disagreements", self.root, self.stub, ["--repo", "demo"]))
        text = capture.getvalue()
        self.assertIn("proven, no truth line: f.py:10  line 10", text)
        self.assertIn("truth line, no site: f.py:15  line 15", text)


def git(*args, cwd):
    done = subprocess.run(
        ["git", "-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", *args],
        cwd=cwd, capture_output=True, text=True, check=True,
    )
    return done.stdout.strip()


class MaterializeTest(unittest.TestCase):
    def test_materialize_pins_the_commit_and_a_second_run_reuses_the_copy(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "source"
            source.mkdir()
            git("init", "-q", "-b", "main", cwd=source)
            (source / "f.txt").write_text("one\n")
            git("add", "f.txt", cwd=source)
            git("commit", "-q", "-m", "one", cwd=source)
            pinned = git("rev-parse", "HEAD", cwd=source)
            (source / "f.txt").write_text("two\n")
            git("commit", "-q", "-am", "two", cwd=source)
            entry = {"name": "demo", "language": "python", "url": "unused", "commit": pinned, "local": str(source)}
            cache = Path(tmp) / "cache"
            path, created = reach_gate.materialize_repo(entry, cache)
            self.assertTrue(created)
            self.assertEqual(path.name, f"demo-{pinned[:12]}")
            self.assertEqual(git("rev-parse", "HEAD", cwd=path), pinned)
            self.assertEqual((path / "f.txt").read_text(), "one\n")
            (path / "marker").write_text("kept")
            again, created_again = reach_gate.materialize_repo(entry, cache)
            self.assertEqual(again, path)
            self.assertFalse(created_again)
            self.assertEqual((path / "marker").read_text(), "kept")

    def test_materialize_falls_back_to_the_url_when_local_is_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "remote"
            source.mkdir()
            git("init", "-q", "-b", "main", cwd=source)
            (source / "f.txt").write_text("one\n")
            git("add", "f.txt", cwd=source)
            git("commit", "-q", "-m", "one", cwd=source)
            pinned = git("rev-parse", "HEAD", cwd=source)
            entry = {"name": "demo", "language": "python", "url": str(source), "commit": pinned, "local": str(Path(tmp) / "absent")}
            path, created = reach_gate.materialize_repo(entry, Path(tmp) / "cache")
            self.assertTrue(created)
            self.assertEqual(git("rev-parse", "HEAD", cwd=path), pinned)


class PreflightStepTest(unittest.TestCase):
    def step(self):
        text = PREFLIGHT.read_text()
        match = re.search(r"(?ms)^# \d+\. Reach gate.*?(?=^# \d+\. Dogfood)", text)
        self.assertIsNotNone(match, "reach gate step is missing from release-preflight.sh")
        return match.group(0)

    def run_step(self, repo):
        script = f"set -euo pipefail\nREPO_ROOT={shlex.quote(str(repo))}\ncd \"$REPO_ROOT\"\n{self.step()}\n"
        return subprocess.run(["bash", "-c", script], capture_output=True, text=True, check=False)

    def repo_with_commit(self, tmp):
        repo = Path(tmp)
        git("init", "-q", "-b", "main", cwd=repo)
        git("commit", "-q", "--allow-empty", "-m", "one", cwd=repo)
        return repo, git("rev-parse", "HEAD", cwd=repo)

    def write_report(self, repo, commit, result):
        report = repo / "target" / "reach-gate" / "report.md"
        report.parent.mkdir(parents=True)
        report.write_text(f"# Reach gate report\n\nCommit: {commit}\n\n## Result: {result}\n")

    def test_step_fails_without_a_report(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, _ = self.repo_with_commit(tmp)
            done = self.run_step(repo)
            self.assertEqual(done.returncode, 1)
            self.assertIn("no report", done.stderr)

    def test_step_fails_on_a_failing_report(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, commit = self.repo_with_commit(tmp)
            self.write_report(repo, commit, "FAIL")
            done = self.run_step(repo)
            self.assertEqual(done.returncode, 1)
            self.assertIn("## Result: PASS", done.stderr)

    def test_step_fails_when_the_report_commit_is_not_an_ancestor_of_head(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, _ = self.repo_with_commit(tmp)
            self.write_report(repo, "0" * 40, "PASS")
            done = self.run_step(repo)
            self.assertEqual(done.returncode, 1)
            self.assertIn("not an ancestor", done.stderr)

    def test_step_passes_with_a_passing_report_from_an_ancestor_commit(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, commit = self.repo_with_commit(tmp)
            self.write_report(repo, commit, "PASS")
            git("commit", "-q", "--allow-empty", "-m", "two", cwd=repo)
            done = self.run_step(repo)
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertIn("OK", done.stdout)


def scip_fixture(root):
    files = {
        "pkg/core.py": "def target(x):\n    return x\n\n\ndef alt(x):\n    return x\n",
        "pkg/use.py": (
            "from pkg.core import (\n"
            "    target,\n"
            ")\n"
            "import pkg.core\n"
            "\n\n"
            "@target\n"
            "def decorated():\n"
            "    pass\n"
            "\n"
            "def call():\n"
            "    target(1)\n"
            "    fn = target\n"
            "    return fn\n"
        ),
        "src/Foo.java": "class Foo {\n  void bar() {}\n  void bar(int x) {}\n}\n",
        "src/Use.java": (
            "import demo.Foo;\n"
            "class Use {\n"
            "  void go(Foo f) {\n"
            "    f.bar();\n"
            "    f.bar(1);\n"
            "  }\n"
            "}\n"
        ),
    }
    for name, text in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(text)
    target = "scip-python python demo 0.1 `pkg.core`/target()."
    alt = "scip-python python demo 0.1 `pkg.core`/alt()."
    bar = "scip-java maven demo 1.0 demo/Foo#bar()."
    bar_int = "scip-java maven demo 1.0 demo/Foo#bar(+1)."
    documents = [
        document_bytes("pkg/core.py", [occurrence_bytes(1, 4, 10, target, 1, (1, 2)), occurrence_bytes(5, 4, 7, alt, 1, (5, 6))]),
        document_bytes(
            "pkg/use.py",
            [occurrence_bytes(2, 4, 10, target), occurrence_bytes(7, 1, 7, target), occurrence_bytes(12, 4, 10, target), occurrence_bytes(13, 9, 15, target)],
        ),
        document_bytes("src/Foo.java", [occurrence_bytes(2, 7, 10, bar, 1), occurrence_bytes(3, 7, 10, bar_int, 1)]),
        document_bytes("src/Use.java", [occurrence_bytes(4, 6, 9, bar), occurrence_bytes(5, 6, 9, bar_int)]),
    ]
    return index_bytes(documents)


class ImportLinesTest(unittest.TestCase):
    def test_python_parenthesised_import_blocks_and_plain_imports(self):
        text = "import os\nfrom a import (\n    b,\n    c,\n)\nx = b\nfrom d import e\n"
        self.assertEqual(reach_truth.import_line_numbers("m.py", text), {1, 2, 3, 4, 5, 7})

    def test_javascript_multiline_imports_and_reexports_but_not_dynamic_import(self):
        text = (
            "import {\n  a,\n  b,\n} from './x'\n"
            "import c from \"y\";\n"
            "export { d } from './z';\n"
            "export { e };\n"
            "const f = import('./w');\n"
        )
        self.assertEqual(reach_truth.import_line_numbers("m.js", text), {1, 2, 3, 4, 5, 6})

    def test_rust_use_statements_over_several_lines(self):
        text = "use std::fmt;\npub use a::{\n    b,\n    c,\n};\nfn f() { b(); }\n"
        self.assertEqual(reach_truth.import_line_numbers("m.rs", text), {1, 2, 3, 4, 5})

    def test_go_import_block_and_single_import(self):
        text = 'package p\nimport "os"\nimport (\n\t"a"\n\t"b"\n)\nfunc f() {}\n'
        self.assertEqual(reach_truth.import_line_numbers("m.go", text), {2, 3, 4, 5, 6})

    def test_csharp_using_directives_but_not_using_statements(self):
        text = "using System;\nusing static A.B;\nusing X = A.C;\nclass K { void f() {\n using (var s = g()) { }\n using var t = h();\n} }\n"
        self.assertEqual(reach_truth.import_line_numbers("m.cs", text), {1, 2, 3})

    def test_java_imports(self):
        self.assertEqual(reach_truth.import_line_numbers("M.java", "import a.B;\nclass M {}\n"), {1})

    def test_unknown_extension_has_no_import_lines(self):
        self.assertEqual(reach_truth.import_line_numbers("notes.txt", "import x\n"), set())


class TruthTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.repo = self.root / "cache" / "demo-cccccccccccc"
        self.repo.mkdir(parents=True)
        self.index_path = self.root / "demo.scip"
        self.index_path.write_bytes(scip_fixture(self.repo))
        self.corpus = self.root / "corpus.json"
        self.corpus.write_text(json.dumps([
            {"name": "demo", "language": "python", "url": "unused", "commit": "c" * 40, "local": None}
        ]))
        self.cases = self.root / "cases"
        self.cases.mkdir()

    def common(self, command, *extra):
        return [
            command, "--repo", "demo", "--index", str(self.index_path), "--corpus", str(self.corpus),
            "--cache-dir", str(self.root / "cache"), *extra,
        ]

    def test_scip_decoder_reads_documents_symbols_and_roles(self):
        _, documents, _ = scip_decode.load(self.index_path)
        self.assertEqual([doc["path"] for doc in documents], ["pkg/core.py", "pkg/use.py", "src/Foo.java", "src/Use.java"])
        first = documents[0]["occurrences"][0]
        self.assertEqual((first["start_line"], first["start_col"], first["end_col"], first["roles"]), (1, 4, 10, 1))
        self.assertEqual((first["enc_start"], first["enc_end"]), (1, 2))

    def test_truth_drops_import_lines_and_keeps_decorator_call_and_value_uses(self):
        index = reach_truth.ScipIndex(self.index_path)
        group = reach_truth.find_group(index, "target", "pkg/core.py", 1)
        self.assertEqual(
            index.truth_lines(group, self.repo),
            [("pkg/use.py", 7), ("pkg/use.py", 12), ("pkg/use.py", 13)],
        )

    def test_overloads_with_the_same_owner_and_name_are_unioned(self):
        index = reach_truth.ScipIndex(self.index_path)
        group = reach_truth.find_group(index, "bar", "src/Foo.java", 3)
        self.assertEqual(len(group.symbols), 2)
        self.assertEqual(index.truth_lines(group, self.repo), [("src/Use.java", 4), ("src/Use.java", 5)])

    def test_path_prefix_is_added_to_every_document_path(self):
        index = reach_truth.ScipIndex(self.index_path, prefix="Src/")
        group = reach_truth.find_group(index, "bar", "Src/src/Foo.java", 2)
        self.assertEqual(group.name, "bar")

    def test_symbol_query_prints_the_truth_lines_of_a_dotted_name(self):
        capture = io.StringIO()
        with contextlib.redirect_stdout(capture):
            code = reach_truth.main(self.common("truth", "--symbol", "Foo.bar"))
        self.assertEqual(code, 0)
        self.assertEqual(
            json.loads(capture.getvalue()),
            [{"path": "src/Use.java", "line": 4}, {"path": "src/Use.java", "line": 5}],
        )

    def test_case_file_truth_is_rebuilt_for_scip_symbols_and_manual_symbols_are_kept(self):
        manual_truth = [{"path": "x.js", "line": 3}]
        adjudicated = [{"path": "pkg/use.py", "line": 13, "verdict": "false", "note": "alias"}]
        case = {
            "repo": "demo", "commit": "c" * 40, "language": "python",
            "symbols": [
                {"id": "t", "split": "dev", "name": "target", "file": "pkg/core.py", "line": 1, "kind": "function",
                 "tags": [], "truth_source": "scip", "truth": [], "adjudicated": adjudicated},
                {"id": "m", "split": "holdout", "name": "hand", "file": "x.js", "line": 1, "kind": "function",
                 "tags": [], "truth_source": "manual", "truth": manual_truth, "adjudicated": []},
            ],
        }
        (self.cases / "demo.json").write_text(json.dumps(case))
        code = reach_truth.main(self.common("truth", "--cases-dir", str(self.cases), "--write"))
        self.assertEqual(code, 0)
        written = json.loads((self.cases / "demo.json").read_text())
        self.assertEqual([row["line"] for row in written["symbols"][0]["truth"]], [7, 12, 13])
        self.assertEqual(written["symbols"][0]["adjudicated"], adjudicated)
        self.assertEqual(written["symbols"][1]["truth"], manual_truth)


def sample_fixture(root):
    specs = [
        ("m1", "run", 2, 0, 0),
        ("m2", "run", 1, 0, 0),
        ("m3", "run", 1, 0, 0),
        ("lib", "busy", 25, 0, 0),
        ("lib", "passed", 1, 2, 0),
        ("lib", "deco", 0, 0, 2),
        ("lib", "plain_a", 1, 0, 0),
        ("lib", "plain_b", 2, 0, 0),
        ("lib", "plain_c", 1, 0, 0),
        ("lib", "plain_d", 3, 0, 0),
        ("lib", "plain_e", 1, 0, 0),
        ("lib", "plain_f", 2, 0, 0),
    ]
    app, defs, refs, lib_lines = [], {}, {}, []
    for module, name, calls, values, decorators in specs:
        symbol = f"scip-python python demo 0.1 `{module}`/{name}()."
        if module == "lib":
            lib_lines.append(f"def {name}(x):")
            lib_lines.append("    return x")
            lib_lines.append("")
            line = len(lib_lines) - 2
        else:
            (root / f"{module}.py").write_text(f"def {name}(x):\n    return x\n")
            line = 1
        defs.setdefault(f"{module}.py", []).append(occurrence_bytes(line, 4, 4 + len(name), symbol, 1))
        uses = refs.setdefault("app.py", [])
        for _ in range(calls):
            app.append(f"{name}(1)")
            uses.append(occurrence_bytes(len(app), 0, len(name), symbol))
        for _ in range(values):
            app.append(f"x = {name}")
            uses.append(occurrence_bytes(len(app), 4, 4 + len(name), symbol))
        for _ in range(decorators):
            app.append(f"@{name}")
            uses.append(occurrence_bytes(len(app), 1, 1 + len(name), symbol))
            app.append("def wrapped(): pass")
    (root / "lib.py").write_text("\n".join(lib_lines) + "\n")
    (root / "app.py").write_text("\n".join(app) + "\n")
    documents = [document_bytes(path, occs) for path, occs in {**defs, **refs}.items()]
    return index_bytes(documents)


class SkippedNameTest(unittest.TestCase):
    def test_accessor_and_dunder_names_are_skipped_and_plain_names_are_not(self):
        for name in ("<get>schema", "<set>value", "<lambda>", "__init__"):
            self.assertTrue(reach_truth.SKIPPED_NAME.match(name), name)
        self.assertFalse(reach_truth.SKIPPED_NAME.match("get"))


class SampleTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self.index_path = self.repo / "demo.scip"
        self.index_path.write_bytes(sample_fixture(self.repo))
        self.entry = {"name": "demo", "language": "python", "url": "unused", "commit": "c" * 40, "local": None}
        self.index = reach_truth.ScipIndex(self.index_path)

    def test_candidates_carry_the_tags_computed_from_scip_and_source(self):
        tags = {row["name"]: row["tags"] for row in reach_truth.candidates(self.index, self.repo)}
        self.assertIn("common-name", tags["run"])
        self.assertIn("many-refs", tags["busy"])
        self.assertIn("value-ref", tags["passed"])
        self.assertIn("decorator", tags["deco"])
        self.assertNotIn("value-ref", tags["deco"])
        self.assertEqual(tags["plain_a"], [])

    def test_generated_build_and_test_files_are_not_sampled(self):
        documents = []
        for folder, name in (("target/gen", "built"), ("tests", "checked"), ("generated-sources", "made"), ("src", "kept")):
            symbol = f"scip-python python demo 0.1 `{name}`/{name}()."
            documents.append(document_bytes(f"{folder}/{name}.py", [occurrence_bytes(1, 4, 4 + len(name), symbol, 1)]))
            documents.append(document_bytes(f"{folder}/use_{name}.py", [occurrence_bytes(1, 0, len(name), symbol)]))
        index_file = self.repo / "paths.scip"
        index_file.write_bytes(index_bytes(documents))
        found = reach_truth.candidates(reach_truth.ScipIndex(index_file), self.repo)
        self.assertEqual([row["name"] for row in found], ["kept"])

    def test_overridden_tag_comes_from_scip_implementation_relationships(self):
        base = "scip-java maven demo 1.0 demo/Base#go()."
        child = "scip-java maven demo 1.0 demo/Child#go()."
        path = self.repo / "Base.java"
        path.write_text("class Base { void go() {} }\n")
        (self.repo / "Child.java").write_text("class Child { void go() {} }\n")
        (self.repo / "Use.java").write_text("class Use { void f() { g.go(); } }\n")
        documents = [
            document_bytes("Base.java", [occurrence_bytes(1, 19, 21, base, 1)]),
            document_bytes("Child.java", [occurrence_bytes(1, 20, 22, child, 1)], [info_bytes(child, implements=[base])]),
            document_bytes("Use.java", [occurrence_bytes(1, 28, 30, base)]),
        ]
        index_file = self.repo / "java.scip"
        index_file.write_bytes(index_bytes(documents))
        found = {row["file"]: row["tags"] for row in reach_truth.candidates(reach_truth.ScipIndex(index_file), self.repo)}
        self.assertIn("overridden", found["Base.java"])
        self.assertNotIn("overridden", found.get("Child.java", []))

    def test_sample_is_deterministic_balanced_and_stratified(self):
        first = reach_truth.sample(self.index, self.repo, self.entry, 6, 20261017)
        second = reach_truth.sample(self.index, self.repo, self.entry, 6, 20261017)
        self.assertEqual(first, second)
        symbols = first["symbols"]
        self.assertEqual(len(symbols), 6)
        self.assertEqual(len({row["id"] for row in symbols}), 6)
        splits = [row["split"] for row in symbols]
        self.assertEqual((splits.count("dev"), splits.count("holdout")), (3, 3))
        chosen_tags = {tag for row in symbols for tag in row["tags"]}
        self.assertTrue({"common-name", "many-refs", "value-ref", "decorator"} <= chosen_tags)
        self.assertEqual(first["repo"], "demo")
        self.assertEqual(first["language"], "python")
        self.assertTrue(all(row["truth_source"] == "scip" and row["truth"] == [] for row in symbols))

    def test_a_different_seed_changes_the_picks(self):
        def ids(seed):
            return [row["id"] for row in reach_truth.sample(self.index, self.repo, self.entry, 8, seed)["symbols"]]

        self.assertTrue(any(ids(seed) != ids(1) for seed in range(2, 8)))

    def test_sample_returns_every_candidate_when_fewer_than_the_count_exist(self):
        found = reach_truth.sample(self.index, self.repo, self.entry, 500, 20261017)
        self.assertEqual(len(found["symbols"]), len(reach_truth.candidates(self.index, self.repo)))


if __name__ == "__main__":
    unittest.main()
