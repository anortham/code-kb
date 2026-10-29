import json
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

import bug_replay


def answer(tests=(), found=None, cut=False, warning=None):
    rows = [{"path": path, "name": name, "reason": reason} for path, name, reason in tests]
    result = {"tests": rows, "likely_tests_found": len(rows) if found is None else found,
              "likely_tests_truncated": cut}
    if warning:
        result[bug_replay.WARNING_FIELD] = warning
    return result


FAILING = [{"path": "tests/test_basic.py", "name": "test_session"},
           {"path": "tests/test_helpers.py", "name": "TestStreaming::test_stream"}]


class TestKeys(unittest.TestCase):
    def test_a_key_drops_classes_parameters_and_subtests(self):
        self.assertEqual(bug_replay.test_key("t.py", "TestStreaming::test_x[a-b]"), ("t.py", "test_x"))
        self.assertEqual(bug_replay.test_key("x_test.go", "TestRun/sub_case"), ("x_test.go", "TestRun"))


class TestRunners(unittest.TestCase):
    def test_pytest_cases_name_their_file_and_classes_and_a_failing_parameter_fails_the_test(self):
        with tempfile.TemporaryDirectory() as tree:
            Path(tree, "tests").mkdir()
            Path(tree, "tests/test_helpers.py").write_text("")
            report = ET.fromstring(
                '<testsuites><testsuite>'
                '<testcase classname="tests.test_helpers.TestStreaming" name="test_stream"/>'
                '<testcase classname="tests.test_helpers" name="test_p[a]"><failure/></testcase>'
                '<testcase classname="tests.test_helpers" name="test_p[b]"/>'
                '</testsuite></testsuites>')

            outcomes = bug_replay.pytest_outcomes(report, tree)

        self.assertEqual(outcomes, {("tests/test_helpers.py", "TestStreaming::test_stream"): "passed",
                                    ("tests/test_helpers.py", "test_p"): "failed"})

    def test_go_events_map_each_top_level_test_to_its_file(self):
        with tempfile.TemporaryDirectory() as tree:
            Path(tree, "cmd_test.go").write_text("package cmd\n\nfunc TestRun(t *testing.T) {}\n")
            events = "\n".join(json.dumps(event) for event in [
                {"Action": "run", "Package": "ex.com/cmd", "Test": "TestRun"},
                {"Action": "fail", "Package": "ex.com/cmd", "Test": "TestRun/sub"},
                {"Action": "pass", "Package": "ex.com/cmd", "Test": "TestRun"},
            ])

            outcomes = bug_replay.go_outcomes(events, {"ex.com/cmd": tree}, Path(tree))

        self.assertEqual(outcomes, {("cmd_test.go", "TestRun"): "failed"})

    def test_a_go_build_failure_is_a_broken_run(self):
        events = json.dumps({"Action": "output", "Package": "ex.com/cmd", "Output": "FAIL ex.com/cmd [build failed]\n"})

        with self.assertRaises(bug_replay.BrokenRun):
            bug_replay.go_outcomes(events, {}, Path("/tmp"))

    def test_mocha_tests_are_keyed_by_their_file_and_title(self):
        with tempfile.TemporaryDirectory() as tree:
            report = {"passes": [{"title": "works", "file": str(Path(tree, "test/app.js"))}],
                      "failures": [{"title": "breaks", "file": str(Path(tree, "test/app.js"))}]}

            outcomes = bug_replay.mocha_outcomes(report, tree)

        self.assertEqual(outcomes, {("test/app.js", "works"): "passed", ("test/app.js", "breaks"): "failed"})

    def test_a_mocha_run_of_one_file_keeps_the_options_and_drops_the_folders(self):
        script = "mocha --require test/support/env --reporter spec --check-leaks test/ test/acceptance/"

        self.assertEqual(bug_replay.mocha_command(script, ["test/res.send.js"]),
                         ["--require", "test/support/env", "--check-leaks", "--exit", "test/res.send.js"])

    def test_an_option_and_its_value_are_dropped_from_a_mocha_command(self):
        self.assertEqual(bug_replay.drop_option(["--reporter", "spec", "--check-leaks", "--reporter=dot", "test/"],
                                                "--reporter"), ["--check-leaks", "test/"])


class TestMutants(unittest.TestCase):
    def test_a_bug_flips_the_first_operator_outside_a_string(self):
        self.assertEqual(bug_replay.mutate('    if s == "a == b":', bug_replay.PYTHON_MUTATIONS),
                         '    if s != "a == b":')
        self.assertEqual(bug_replay.mutate("\tif ok && done {", bug_replay.C_LIKE_MUTATIONS), "\tif ok || done {")
        self.assertIsNone(bug_replay.mutate("    pass", bug_replay.PYTHON_MUTATIONS))

    def test_bugs_land_on_the_source_lines_the_commit_added(self):
        with tempfile.TemporaryDirectory() as tree:
            def git(*args):
                subprocess.run(["git", "-c", "user.name=T", "-c", "user.email=t@t", "-c", "commit.gpgsign=false",
                                *args], cwd=tree, check=True, capture_output=True)
            git("init", "-q")
            Path(tree, "src").mkdir()
            Path(tree, "src/a.py").write_text("def f(x):\n    return x\n")
            git("add", ".")
            git("commit", "-q", "-m", "base")
            Path(tree, "src/a.py").write_text("def f(x):\n    if x == 1:\n        return True\n    return x\n")
            Path(tree, "tests").mkdir()
            Path(tree, "tests/test_a.py").write_text("def test_f():\n    assert 1 == 1\n")
            git("add", ".")
            git("commit", "-q", "-m", "change")
            commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=tree, capture_output=True, text=True, check=True).stdout.strip()

            found = bug_replay.mutants(Path(tree), commit, bug_replay.Pytest)

        self.assertEqual({(m["path"], m["line"]) for m in found[:2]}, {("src/a.py", 2), ("src/a.py", 3)})
        self.assertTrue(all(m["path"] == "src/a.py" for m in found))


class TestJudge(unittest.TestCase):
    def test_a_list_that_misses_a_failing_test_without_a_warning_is_a_silent_miss(self):
        judged = bug_replay.judge(answer([("tests/test_basic.py", "test_session", "direct caller")]), FAILING)

        self.assertEqual((judged["tests_missed"], judged["files_missed"]), (1, 1))
        self.assertTrue(judged["silent_miss"])
        self.assertFalse(judged["needless_warning"])

    def test_a_warned_or_cut_or_empty_or_depth_cut_answer_is_no_silent_miss(self):
        warned = bug_replay.judge(answer([("tests/test_basic.py", "test_session", "direct caller")],
                                         warning=[{"name": "__call__"}]), FAILING)
        cut = bug_replay.judge(answer([("tests/test_basic.py", "test_session", "direct caller")], found=300,
                                      cut=True), FAILING)
        empty = bug_replay.judge(answer(), FAILING)
        deep = answer([("tests/test_basic.py", "test_session", "direct caller")])
        deep.update(stopped_at_depth=2, tests_past_depth=3)
        stopped = bug_replay.judge(deep, FAILING)

        self.assertFalse(warned["silent_miss"] or cut["silent_miss"] or empty["silent_miss"] or stopped["silent_miss"])
        self.assertTrue(stopped["warned"])
        self.assertTrue(empty["empty"])
        self.assertFalse(warned["needless_warning"])

    def test_a_depth_cut_that_a_deeper_walk_adds_no_test_to_is_no_warning(self):
        deep = answer([("tests/test_basic.py", "test_session", "direct caller")])
        deep.update(stopped_at_depth=2, tests_past_depth=0)

        self.assertTrue(bug_replay.judge(deep, FAILING)["silent_miss"])

    def test_a_warning_on_a_complete_list_is_needless(self):
        judged = bug_replay.judge(answer([("tests/test_basic.py", "test_session", "direct caller"),
                                          ("tests/test_helpers.py", "TestStreaming::test_stream", "direct caller")],
                                         warning=[{"name": "__call__"}]), FAILING)

        self.assertEqual(judged["tests_missed"], 0)
        self.assertTrue(judged["needless_warning"])

    def test_a_folded_constructor_row_covers_its_whole_file(self):
        judged = bug_replay.judge(answer([("tests/test_basic.py", "test_session", "direct caller"),
                                          ("tests/test_helpers.py", "tests/test_helpers.py",
                                           "4 tests reach it only through a constructor")]), FAILING)

        self.assertEqual(judged["tests_missed"], 0)


class TestScore(unittest.TestCase):
    def test_each_repository_and_seed_gets_its_own_row(self):
        truth = [{"repo": "flask", "commit": "c1", "outcome": "detected", "suite_tests": 10, "failing": FAILING},
                 {"repo": "flask", "commit": "c2", "outcome": "undetected", "suite_tests": 10, "failing": []}]
        records = [{"repo": "flask", "commit": commit,
                    "symbol_answer": answer([("tests/test_basic.py", "test_session", "direct caller")]),
                    "diff_answer": answer()} for commit in ("c1", "c2")]

        summary = bug_replay.score(records, truth)

        self.assertEqual(summary[("flask", "symbol")]["bugs"], 1)
        self.assertEqual(summary[("flask", "symbol")]["silent_misses"], 1)
        self.assertEqual(summary[("flask", "symbol")]["median_share"], 0.1)
        self.assertEqual(summary[("flask", "diff")]["empty"], 1)
        self.assertEqual(summary[("flask", "diff")]["bugs_fully_missed"], 1)


if __name__ == "__main__":
    unittest.main()
