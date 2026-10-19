import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import edit_replay

DEV_SHA = "00000002" + "a" * 32
HOLDOUT_SHA = "00000003" + "b" * 32


def site(path, line, caller="caller"):
    return {"path": path, "line": line, "caller": caller}


def change(display="area", path="src/geo.py", proven=(), likely=(), text="parameters 2 → 3"):
    return {"kind": "function", "display": display, "path": path, "line": 1, "change": text, "in_diff": 1,
            "proven": list(proven), "likely": list(likely), "unresolved": 0}


def record(commit, changes, hook=None, status="ok", language="python", seconds=1.0):
    return {"repo": "flask", "language": language, "commit": commit, "parent": "p" + commit[1:],
            "split": edit_replay.split_of(commit), "status": status, "seconds": seconds,
            "changes": changes, "hook": hook}


def case(commit, verdicts=(), language="python"):
    return {"repo": "flask", "language": language, "commit": commit, "parent": "p" + commit[1:],
            "split": edit_replay.split_of(commit), "verdicts": list(verdicts)}


def verdict(path, line, value, display="area"):
    return {"path": path, "line": line, "display": display, "verdict": value}


def hook_block(*rows):
    return {"block": True, "count": len(rows), "ran": True,
            "rows": [{"path": p, "line": n, "display": "area", "change": "parameters 2 → 3"} for p, n in rows]}


def git(cwd, *args):
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True,
                   env={**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t",
                        "GIT_COMMITTER_EMAIL": "t@t"})


class SplitTest(unittest.TestCase):
    def test_an_even_first_word_is_dev_and_an_odd_one_is_holdout(self):
        self.assertEqual(edit_replay.split_of(DEV_SHA), "dev")
        self.assertEqual(edit_replay.split_of(HOLDOUT_SHA), "holdout")


class BlockRuleTest(unittest.TestCase):
    def test_listed_rows_are_every_proven_and_likely_site_outside_the_diff(self):
        rec = record(DEV_SHA, [change(proven=[site("a.py", 3)], likely=[site("b.py", 9)]), change(display="x")])
        rows = edit_replay.listed_block_rows(rec)
        self.assertEqual([(r["path"], r["line"], r["label"]) for r in rows], [("a.py", 3, "proven"), ("b.py", 9, "likely")])

    def test_unresolved_sites_and_in_diff_sites_never_list(self):
        quiet = change()
        quiet["unresolved"] = 4
        self.assertEqual(edit_replay.listed_block_rows(record(DEV_SHA, [quiet])), [])
        self.assertFalse(edit_replay.hook_can_block(record(DEV_SHA, [quiet])))

    def test_a_block_with_one_true_row_is_true(self):
        rows = [{"path": "a.py", "line": 3, "display": "area", "change": "removed"},
                {"path": "b.py", "line": 9, "display": "area", "change": "removed"}]
        verdicts = {("a.py", 3, "area"): "false", ("b.py", 9, "area"): "true"}
        self.assertEqual(edit_replay.judge_block(rows, verdicts), "true")

    def test_a_block_is_false_only_when_every_row_is_judged_false(self):
        rows = [{"path": "a.py", "line": 3, "display": "area", "change": "removed"},
                {"path": "b.py", "line": 9, "display": "area", "change": "removed"}]
        self.assertEqual(edit_replay.judge_block(rows, {("a.py", 3, "area"): "false"}), "unjudged")
        self.assertEqual(edit_replay.judge_block(rows, {("a.py", 3, "area"): "false", ("b.py", 9, "area"): "false"}),
                         "false")


class HookOutputTest(unittest.TestCase):
    def test_no_output_is_no_block(self):
        self.assertEqual(edit_replay.parse_hook_output(""), {"block": False, "count": 0, "rows": []})

    def test_a_block_lists_its_sites_by_path_and_line(self):
        reason = ("Before you finish: 2 sites still use the old shape of `area`.\n"
                  "src/a b.rs:8  area: parameters 2 → 3\nsrc/c.rs:12  area: parameters 2 → 3\n"
                  "Edit them or say why they need no change.")
        parsed = edit_replay.parse_hook_output(json.dumps({"decision": "block", "reason": reason}))
        self.assertTrue(parsed["block"])
        self.assertEqual(parsed["count"], 2)
        self.assertEqual([(r["path"], r["line"], r["display"]) for r in parsed["rows"]],
                         [("src/a b.rs", 8, "area"), ("src/c.rs", 12, "area")])


class FakeBinaryTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def fake(self, body):
        path = self.dir / "code-kb"
        path.write_text("#!/bin/sh\n" + body)
        path.chmod(0o755)
        return path

    def test_a_check_that_exits_nonzero_is_an_error(self):
        binary = self.fake("echo boom >&2\nexit 3\n")
        self.assertEqual(edit_replay.run_check(binary, self.dir, "HEAD~1")["status"], "error")

    def test_a_check_keeps_the_sites_of_each_change(self):
        report = {"files_changed": 2, "cuts": [], "changes": [change(proven=[site("a.py", 3)]) | {"in_diff": [site("c.py", 1)]}]}
        binary = self.fake(f"cat <<'EOF'\n{json.dumps(report)}\nEOF\n")
        got = edit_replay.run_check(binary, self.dir, "HEAD~1")
        self.assertEqual(got["status"], "ok")
        self.assertEqual(got["changes"][0]["in_diff"], 1)
        self.assertEqual(got["changes"][0]["proven"], [site("a.py", 3)])

    def test_the_stop_hook_runs_with_a_session_base_and_cleans_up(self):
        script = (
            'cat > /dev/null\n'
            'f=.code-kb/sessions/replay-1.json\n'
            'grep -q \'"base": "abc"\' "$f" || exit 0\n'
            'echo \'{"base":"abc","checked_diff":"h"}\' > "$f"\n'
            'echo \'{"decision":"block","reason":"Before you finish: 1 sites still use the old shape of `f`.\\n'
            'a.py:3  f: removed\\nEdit them or say why they need no change."}\'\n')
        got = edit_replay.run_stop_hook(self.fake(script), self.dir, "abc", "replay-1")
        self.assertTrue(got["ran"])
        self.assertTrue(got["block"])
        self.assertEqual(got["rows"][0]["path"], "a.py")
        self.assertEqual(list((self.dir / ".code-kb/sessions").iterdir()), [])

    def test_a_hook_that_never_checks_is_reported_as_not_run(self):
        got = edit_replay.run_stop_hook(self.fake("cat > /dev/null\n"), self.dir, "abc", "replay-2")
        self.assertFalse(got["ran"])
        self.assertFalse(got["block"])


class WalkTest(unittest.TestCase):
    def test_the_walk_skips_merge_commits_and_root_commits(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init", "-q", "-b", "main")
            for name in ("a", "b"):
                (repo / name).write_text(name)
                git(repo, "add", name)
                git(repo, "commit", "-q", "-m", name)
            git(repo, "checkout", "-q", "-b", "side", "HEAD~1")
            (repo / "c").write_text("c")
            git(repo, "add", "c")
            git(repo, "commit", "-q", "-m", "c")
            git(repo, "checkout", "-q", "main")
            git(repo, "merge", "-q", "--no-ff", "-m", "merge", "side")
            walked = list(edit_replay.walk_commits(repo, "HEAD", 10))
            self.assertEqual(len(walked), 2)
            for commit, parent in walked:
                self.assertEqual(edit_replay.git(repo, "rev-parse", f"{commit}^"), parent)


class ScoreTest(unittest.TestCase):
    def fixture(self):
        good, bad, open_, quiet, broken = (f"{n:08x}" + "c" * 32 for n in (2, 4, 6, 8, 10))
        records = [
            record(good, [change(proven=[site("a.py", 3)])], hook=hook_block(("a.py", 3)), seconds=1.0),
            record(bad, [change(likely=[site("b.py", 5)])], seconds=2.0),
            record(open_, [change(proven=[site("c.py", 7)])], hook=hook_block(("c.py", 7)), seconds=3.0),
            record(quiet, [], seconds=4.0),
            record(broken, [], status="error"),
        ]
        cases = [
            case(good, [verdict("a.py", 3, "true")]),
            case(bad, [verdict("b.py", 5, "false")]),
            case(open_),
            case(quiet),
            case(broken),
        ]
        return records, cases

    def test_score_counts_shape_commits_blocks_and_false_blocks(self):
        records, cases = self.fixture()
        tallies, seconds, todo = edit_replay.score_clean(records, cases)
        python = tallies["python"]
        self.assertEqual((python["commits"], python["errors"], python["shape"]), (5, 1, 3))
        self.assertEqual((python["hook_blocks"], python["hook_false"], python["hook_unjudged"]), (2, 0, 1))
        self.assertEqual((python["listed_blocks"], python["listed_false"], python["listed_unjudged"]), (3, 1, 1))
        self.assertEqual(python["false_rows"], 1)
        self.assertEqual(sorted(seconds), [1.0, 2.0, 3.0, 4.0])
        self.assertEqual([(g["display"], g["rows"], g["hook"], g["samples"][0]["path"]) for g in todo],
                         [("area", 1, True, "c.py")])

    def test_a_group_verdict_judges_every_row_of_its_change(self):
        commit = DEV_SHA
        rows = [site("a.py", n) for n in range(1, 8)]
        records = [record(commit, [change(proven=rows, text="signature changed")])]
        cases = [case(commit, [{"display": "area", "change": "signature changed", "verdict": "false"}])]
        tallies, _, todo = edit_replay.score_clean(records, cases)
        self.assertEqual((tallies["python"]["listed_false"], tallies["python"]["false_rows"]), (1, 7))
        self.assertEqual(todo, [])

    def test_errors_and_unjudged_hook_blocks_leave_the_result_incomplete(self):
        records, cases = self.fixture()
        text = edit_replay.render_clean(*edit_replay.score_clean(records, cases)[:2], "dev", "code-kb test")
        self.assertIn("Result: INCOMPLETE", text)
        self.assertIn("p50 2.00 s", text)

    def test_a_false_hook_block_over_the_mark_fails(self):
        commits = [f"{2 * n:08x}" + "d" * 32 for n in range(1, 11)]
        records = [record(commits[0], [change(proven=[site("a.py", 1)])], hook=hook_block(("a.py", 1)))]
        records += [record(c, [change()]) for c in commits[1:]]
        cases = [case(commits[0], [verdict("a.py", 1, "false")])] + [case(c) for c in commits[1:]]
        text = edit_replay.render_clean(*edit_replay.score_clean(records, cases)[:2], "dev", "code-kb test")
        self.assertIn("| total | 10 | 0 | 10 | 1 | 1 | 10.0% |", text)
        self.assertIn("Result: FAIL", text)

    def test_no_false_hook_block_passes(self):
        records, cases = self.fixture()
        records = [r for r in records if r["status"] == "ok" and r["commit"] != records[2]["commit"]]
        text = edit_replay.render_clean(*edit_replay.score_clean(records, cases)[:2], "dev", "code-kb test")
        self.assertIn("Result: PASS", text)


class VerdictsTest(unittest.TestCase):
    def test_verdicts_merge_into_the_case_and_replace_an_earlier_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            cases_path, verdicts_path = Path(tmp) / "clean.json", Path(tmp) / "v.json"
            edit_replay.write_json(cases_path, {"cases": [case(DEV_SHA, [verdict("a.py", 3, "true")])]})
            edit_replay.write_json(verdicts_path, [
                {"commit": DEV_SHA, "path": "a.py", "line": 3, "display": "area", "change": "removed",
                 "label": "proven", "verdict": "false", "evidence": "a later commit keeps the call"},
            ])
            with contextlib.redirect_stdout(io.StringIO()):
                code = edit_replay.main(["verdicts", "clean", "--cases", str(cases_path), "--from", str(verdicts_path)])
            self.assertEqual(code, 0)
            stored = edit_replay.load_json(cases_path)["cases"][0]["verdicts"]
            self.assertEqual(len(stored), 1)
            self.assertEqual(stored[0]["verdict"], "false")

    def test_a_group_verdict_merges_beside_row_verdicts(self):
        with tempfile.TemporaryDirectory() as tmp:
            cases_path, verdicts_path = Path(tmp) / "clean.json", Path(tmp) / "v.json"
            edit_replay.write_json(cases_path, {"cases": [case(DEV_SHA, [verdict("a.py", 3, "true")])]})
            edit_replay.write_json(verdicts_path, [{"commit": DEV_SHA, "display": "area", "change": "signature changed",
                                                    "verdict": "false", "evidence": "a nullable type"}])
            with contextlib.redirect_stdout(io.StringIO()):
                edit_replay.main(["verdicts", "clean", "--cases", str(cases_path), "--from", str(verdicts_path)])
            stored = edit_replay.load_json(cases_path)["cases"][0]["verdicts"]
            self.assertEqual(len(stored), 2)
            self.assertNotIn("path", [v for v in stored if v["verdict"] == "false"][0])

    def test_a_verdict_other_than_true_or_false_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            cases_path, verdicts_path = Path(tmp) / "clean.json", Path(tmp) / "v.json"
            edit_replay.write_json(cases_path, {"cases": [case(DEV_SHA)]})
            edit_replay.write_json(verdicts_path, [{"commit": DEV_SHA, "path": "a.py", "line": 3, "display": "area",
                                                    "verdict": "maybe"}])
            with contextlib.redirect_stderr(io.StringIO()):
                code = edit_replay.main(["verdicts", "clean", "--cases", str(cases_path), "--from", str(verdicts_path)])
            self.assertEqual(code, 2)
            self.assertEqual(edit_replay.load_json(cases_path)["cases"][0]["verdicts"], [])


class DefinitionTest(unittest.TestCase):
    def test_each_language_reads_its_definition_lines(self):
        samples = [
            ("python", "    def area(self, w, h):", ("callable", "area", "(self,w,h)")),
            ("python", "class Shape(Base):", ("type", "Shape", "")),
            ("javascript", "res.send = function send(body) {", ("callable", "send", "(body)")),
            ("typescript", "export function parse<T>(input: string, opts?: O): T {", ("callable", "parse", "(input:string,opts?:O)")),
            ("typescript", "export type Infer<T> = T;", ("type", "Infer", "")),
            ("java", "  public static <T> T fromJson(String json, Class<T> type) throws X {",
             ("callable", "fromJson", "(Stringjson,Class<T>type)")),
            ("csharp", "    public override string ToString()", ("callable", "ToString", "()")),
            ("go", "func (c *Command) AddCommand(cmds ...*Command) {", ("callable", "AddCommand", "(cmds...*Command)")),
            ("rust", "pub(crate) async fn check(root: &Path) -> Result<()> {", ("callable", "check", "(root:&Path)")),
            ("rust", "pub enum Kind {", ("type", "Kind", "")),
        ]
        for language, line, expected in samples:
            with self.subTest(line=line):
                self.assertEqual(edit_replay.definitions(language, line), [expected])

    def test_statements_are_not_method_heads(self):
        for line in ("    return compute(a);", "    throw new IllegalStateException(x);", "  } else if (ok) {",
                     "    String s = compute(a);", "    foo.bar(x);", "    yield return Next(x);"):
            with self.subTest(line=line):
                self.assertEqual(edit_replay.definitions("java", line), [])


def file_diff(removed, added):
    return {"old": "a.py", "new": "a.py", "removed": list(enumerate(removed, 1)), "added": list(enumerate(added, 1))}


class ShapeChangeTest(unittest.TestCase):
    def test_a_removed_definition_and_a_new_parameter_list_are_changes(self):
        diff = file_diff(["def old(a):", "def area(w, h):"], ["def area(w, h, d):"])
        got = {c["symbol"]: c["change"] for c in edit_replay.shape_changes("python", diff)}
        self.assertEqual(got, {"old": "removed or renamed", "area": "parameters changed"})

    def test_a_moved_definition_and_a_changed_class_head_are_not_changes(self):
        diff = file_diff(["def area(w, h):", "class Shape(A):"], ["def area(w, h):  # moved", "class Shape(B):"])
        self.assertEqual(edit_replay.shape_changes("python", diff), [])


class TruthLineTest(unittest.TestCase):
    def test_a_line_whose_call_changed_is_truth(self):
        diff = file_diff(["x = area(1, 2)", "y = area(3, 4)"], ["x = area(1, 2, 0)", "y = area(3, 4)"])
        self.assertEqual(edit_replay.truth_lines(diff, "area"), [1])

    def test_a_line_that_changed_for_another_reason_is_not_truth(self):
        diff = file_diff(["v = d.get(k) + 1"], ["v = d.get(k) + 2"])
        self.assertEqual(edit_replay.truth_lines(diff, "get"), [])

    def test_a_renamed_use_and_a_bare_reference_are_truth_but_a_comment_is_not(self):
        diff = file_diff(["from geo import area", "# area is old", "f = area"], ["from geo import surface", "f = surface"])
        self.assertEqual(edit_replay.truth_lines(diff, "area"), [1, 3])


class HoldbackSelectorTest(unittest.TestCase):
    def test_the_selector_finds_a_definition_change_and_its_changed_callers(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init", "-q", "-b", "main")
            (repo / "geo.py").write_text("def area(w, h):\n    return w * h\n")
            (repo / "use.py").write_text("from geo import area\n\nx = area(1, 2)\ny = d.get(k) + 1\n")
            (repo / "notes.md").write_text("area(1, 2)\n")
            git(repo, "add", ".")
            git(repo, "commit", "-q", "-m", "one")
            (repo / "geo.py").write_text("def area(w, h, d):\n    return w * h * d\n")
            (repo / "use.py").write_text("from geo import area\n\nx = area(1, 2, 3)\ny = d.get(k) + 2\n")
            (repo / "notes.md").write_text("area(1, 2, 3)\n")
            git(repo, "commit", "-q", "-am", "two")
            commit, parent = next(edit_replay.walk_commits(repo, "HEAD", 1))
            diff = edit_replay.git(repo, "diff", "-U0", "--no-color", parent, commit, "--", "*.py")
            cases = edit_replay.holdback_cases("demo", "python", commit, parent, diff)
            self.assertEqual(len(cases), 1)
            got = cases[0]
            self.assertEqual((got["file"], got["symbol"], got["line"], got["change"]),
                             ("geo.py", "area", 1, "parameters changed"))
            self.assertEqual(got["truth"], [{"path": "use.py", "line": 3}])
            self.assertEqual(got["split"], edit_replay.split_of(commit))


def holdback_record(case_id, caught=(), missed=(), outside=(), shape=True, status="ok", language="python"):
    return {"id": case_id, "repo": "flask", "commit": DEV_SHA, "parent": "p", "split": "dev", "file": "geo.py",
            "symbol": "area", "language": language, "status": status, "shape_seen": shape, "unresolved": 2,
            "caught": [{"path": p, "line": n, "label": "proven"} for p, n in caught],
            "missed": [{"path": p, "line": n, "cause": c} for p, n, c in missed],
            "outside": [{"path": p, "line": n, "label": "likely"} for p, n in outside]}


class HoldbackScoreTest(unittest.TestCase):
    def test_a_miss_takes_the_label_of_the_site_that_covers_it(self):
        answer = {"sites": [{"path": "u.py", "start_line": 4, "label": "unresolved", "basis": "2 definitions"},
                            {"path": "v.py", "start_line": 9, "label": "excluded", "basis": "another type"},
                            {"path": "w.py", "start_line": 2, "label": "proven", "basis": "import"}]}
        self.assertEqual(edit_replay.miss_cause(answer, [], "u.py", 4), ("site labeled unresolved", "2 definitions"))
        self.assertEqual(edit_replay.miss_cause(answer, [], "v.py", 9), ("site excluded", "another type"))
        self.assertEqual(edit_replay.miss_cause(answer, [], "x.py", 1), ("site not indexed", None))
        self.assertEqual(edit_replay.miss_cause(answer, [], "w.py", 2), ("other", "proven"))
        self.assertEqual(edit_replay.miss_cause(answer, ["10 symbols"], "w.py", 2)[0], "cut by a limit")

    def test_score_counts_caught_sites_misses_by_cause_and_outside_verdicts(self):
        cases = [
            {"id": "c1", "split": "dev", "truth": [{"path": "u.py", "line": 3}, {"path": "u.py", "line": 8}],
             "verdicts": [{"path": "o.py", "line": 1, "verdict": "true"}]},
            {"id": "c2", "split": "dev", "truth": [{"path": "v.py", "line": 5}], "verdicts": []},
            {"id": "c3", "split": "dev", "truth": [{"path": "w.py", "line": 5}], "verdicts": []},
        ]
        records = [
            holdback_record("c1", caught=[("u.py", 3)], missed=[("u.py", 8, "site excluded")],
                            outside=[("o.py", 1), ("o.py", 2)]),
            holdback_record("c2", missed=[("v.py", 5, "shape not seen")], shape=False),
            holdback_record("c3", status="error"),
        ]
        tallies, causes, todo = edit_replay.score_holdback(records, cases)
        python = tallies["python"]
        self.assertEqual((python["cases"], python["errors"], python["truth"], python["listed"], python["neither"]),
                         (3, 1, 3, 1, 2))
        self.assertEqual((python["no_shape"], python["outside"], python["outside_true"]), (1, 2, 1))
        self.assertEqual(causes, {"site excluded": 1, "shape not seen": 1})
        self.assertEqual([(row["path"], row["line"]) for row in todo], [("o.py", 2)])
        text = edit_replay.render_holdback(tallies, causes, "dev", "code-kb test")
        self.assertIn("Result: INCOMPLETE", text)

    def test_a_language_with_ten_truth_sites_under_the_catch_mark_fails(self):
        cases = [{"id": f"c{n}", "split": "dev", "truth": [{"path": "u.py", "line": n}], "verdicts": []} for n in range(10)]
        records = [holdback_record(f"c{n}", caught=[("u.py", n)] if n < 7 else [],
                                   missed=[] if n < 7 else [("u.py", n, "site not indexed")]) for n in range(10)]
        text = edit_replay.render_holdback(*edit_replay.score_holdback(records, cases)[:2], "dev", "code-kb test")
        self.assertIn("| python | 10 | 0 | 10 | 7 | 3 | 70.0% |", text)
        self.assertIn("Result: FAIL", text)


def claude_log(root, fail_text):
    entries = [
        {"type": "user", "cwd": root, "gitBranch": "main", "timestamp": "2099-01-01T00:00:00Z",
         "message": {"content": "change area"}},
        {"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "Edit", "input": {
                "file_path": f"{root}/geo.py", "old_string": "def area(w, h):", "new_string": "def area(w, h, d):",
                "replace_all": False}},
            {"type": "tool_use", "id": "t2", "name": "Edit", "input": {
                "file_path": f"{root}/geo.py", "old_string": "not in the file", "new_string": "x"}}]}},
        {"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "ok"},
            {"type": "tool_result", "tool_use_id": "t2", "content": "no match", "is_error": True}]}},
        {"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "t3", "name": "Bash", "input": {"command": "python -m pytest"}}]}},
        {"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t3", "content": fail_text}]}},
        {"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "t4", "name": "Write", "input": {"file_path": f"{root}/late.py", "content": "x"}}]}},
        {"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t4", "content": "ok"}]}},
    ]
    return [json.dumps(entry) for entry in entries]


def codex_log(root, commit, fail_text):
    diff = "@@ -1,2 +1,2 @@\n-def area(w, h):\n+def area(w, h, d):\n     return w * h\n"
    entries = [
        {"type": "session_meta", "payload": {"cwd": root, "timestamp": "2099-01-01T00:00:00Z",
                                             "git": {"commit_hash": commit, "branch": "main"}}},
        {"type": "event_msg", "payload": {"type": "item_completed", "item": {
            "type": "FileChange", "status": "completed", "changes": {f"{root}/geo.py": {"type": "update", "unified_diff": diff}}}}},
        {"type": "event_msg", "payload": {"type": "item_completed", "item": {
            "type": "CommandExecution", "command": ["bash", "-lc", "python -m pytest"], "aggregated_output": fail_text}}},
    ]
    return [json.dumps(entry) for entry in entries]


class SessionReplayTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name) / "proj"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        (self.repo / "geo.py").write_text("def area(w, h):\n    return w * h\n")
        (self.repo / "use.py").write_text("from geo import area\nx = area(1, 2)\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-q", "-m", "one")
        self.commit = edit_replay.git(self.repo, "rev-parse", "HEAD")
        self.fail = 'Traceback (most recent call last):\n  File "use.py", line 2, in <module>\nTypeError: area()'

    def tearDown(self):
        self.tmp.cleanup()

    def test_a_claude_log_replays_the_edits_that_succeeded_before_the_failing_check(self):
        root = str(self.repo)
        session = edit_replay.parse_claude_log(claude_log(root, self.fail), 5)
        self.assertEqual((session["cwd"], session["branch"]), (root, "main"))
        self.assertEqual(len(session["edits"]), 1)
        self.assertEqual(session["commands"], [])
        self.assertIn("TypeError", session["fail_text"])
        overlay = edit_replay.replay_edits(self.repo, self.commit, session["edits"], [root], root)
        self.assertEqual(overlay, {"geo.py": "def area(w, h, d):\n    return w * h\n"})

    def test_a_codex_log_replays_its_patches_on_the_recorded_commit(self):
        root = str(self.repo)
        session = edit_replay.parse_codex_log(codex_log(root, self.commit, self.fail), 3)
        self.assertEqual(session["commit"], self.commit)
        self.assertEqual(edit_replay.start_candidates(self.repo, session)[0], self.commit)
        overlay = edit_replay.replay_edits(self.repo, self.commit, session["edits"], [root], root)
        self.assertEqual(overlay, {"geo.py": "def area(w, h, d):\n    return w * h\n"})

    def test_an_edit_that_does_not_fit_the_commit_rebuilds_nothing(self):
        root = str(self.repo)
        (self.repo / "geo.py").write_text("def area(a, b):\n    return a * b\n")
        git(self.repo, "commit", "-q", "-am", "two")
        later = edit_replay.git(self.repo, "rev-parse", "HEAD")
        session = edit_replay.parse_codex_log(codex_log(root, self.commit, self.fail), 3)
        self.assertIsNone(edit_replay.replay_edits(self.repo, later, session["edits"], [root], root))

    def test_the_failing_output_names_a_site_in_an_unedited_file(self):
        locations = edit_replay.failing_locations(self.fail + "\n --> src/lib.rs:8:5\n")
        self.assertEqual(locations, [("src/lib.rs", 8), ("use.py", 2)])
        tracked = {"use.py", "crates/core/src/lib.rs"}
        self.assertEqual(edit_replay.map_reported_path("src/lib.rs", ["/r"], "/r", tracked), "crates/core/src/lib.rs")
        self.assertEqual(edit_replay.map_reported_path("/r/use.py", ["/r"], "/r", tracked), "use.py")

    def test_a_git_state_command_or_a_shell_edit_blocks_a_rebuild(self):
        self.assertEqual(edit_replay.session_blocker({"commands": ["git stash"]}), "git state changed")
        self.assertEqual(edit_replay.session_blocker({"commands": ["sed -i 's/a/b/' src/x.rs"]}), "shell edit")
        self.assertIsNone(edit_replay.session_blocker({"commands": ["git commit -m x", "cargo test"]}))

    def test_hunks_apply_by_content_when_the_stated_line_is_off(self):
        text = "a\nb\nc\nd\n"
        self.assertEqual(edit_replay.apply_hunks(text, "@@ -9,2 +9,2 @@\n b\n-c\n+C\n"), "a\nb\nC\nd\n")
        self.assertIsNone(edit_replay.apply_hunks(text, "@@ -1,1 +1,1 @@\n-zzz\n+y\n"))


class SessionScoreTest(unittest.TestCase):
    def test_score_counts_rebuilt_and_caught_cases_against_each_mark(self):
        records = [
            {"group": "verified", "status": "ok", "caught": True},
            {"group": "verified", "status": "ok", "caught": False, "cause": "shape not seen"},
            {"group": "missed_B", "status": "ok", "caught": True},
            {"group": "missed_B", "status": "not rebuilt", "cause": "repository gone"},
        ]
        tallies = edit_replay.score_sessions(records)
        self.assertEqual((tallies["verified"]["rebuilt"], tallies["verified"]["caught"]), (2, 1))
        self.assertEqual(tallies["missed_B"]["not_rebuilt"], {"repository gone": 1})
        text = edit_replay.render_sessions(tallies, "dev", "code-kb test")
        self.assertIn("Verified: 1 of 2 rebuilt caught.", text)
        self.assertIn("Result: FAIL", text)


if __name__ == "__main__":
    unittest.main()
