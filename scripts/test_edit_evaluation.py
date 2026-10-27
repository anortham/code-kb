import json
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.edit_evaluation import (
    DEFAULT_CORPUS,
    EvaluationError,
    _expected_test_execution,
    _git_env,
    _run_command,
    hash_tree,
    prepare_case_readiness,
    replay_and_grade,
    run_fake_agent,
    summarize_campaign,
    validate_case,
    validate_manifest,
)


class EditEvaluationOutputTests(unittest.TestCase):
    def test_colored_vitest_summary_is_detected_but_empty_run_is_not(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / "project"
            project.mkdir()
            stdout = root / "stdout.log"
            stderr = root / "stderr.log"
            captured_output = (
                "\x1b[2m Test Files \x1b[22m \x1b[1m\x1b[32m2 passed\x1b[39m\x1b[22m\x1b[90m (2)\x1b[39m\n"
                "\x1b[2m      Tests \x1b[22m \x1b[1m\x1b[32m34 passed\x1b[39m\x1b[22m\x1b[2m | \x1b[22m\x1b[33m1 skipped\x1b[39m\x1b[90m (35)\x1b[39m\n"
            )
            stdout.write_text(captured_output)
            stderr.write_text("")
            sandbox_result = {
                "exit_status": 0,
                "timed_out": False,
                "elapsed_seconds": 0.1,
                "process_cleanup": {"attempted": False, "complete": True, "signal": None},
                "stdout_log": str(stdout),
                "stderr_log": str(stderr),
            }
            command = {
                "id": "baseline",
                "argv": ["pnpm", "vitest", "run"],
                "cwd": ".",
                "timeout_seconds": 10,
                "expected_output": ["Test Files", "2 passed", "Tests", "34 passed"],
            }
            with patch("scripts.edit_evaluation.edit_sandbox.run", return_value=sandbox_result):
                result = _run_command(command, project, root / "state", root / "evidence", "grade", {}, {})

            self.assertTrue(result["test_execution_observed"])
            self.assertEqual(result["expected_output_missing"], [])
            self.assertTrue(result["passed"])
            self.assertEqual(stdout.read_text(), captured_output)
            empty_run = "\x1b[2m Test Files \x1b[22m \x1b[1m\x1b[33m0 passed\x1b[39m\x1b[22m\x1b[90m (0)\x1b[39m\n"
            self.assertFalse(_expected_test_execution(command, empty_run))


class EditEvaluationCaseTests(unittest.TestCase):
    def test_reference_test_files_are_validated_without_becoming_protected_or_production(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            case_dir = root / "case"
            case_dir.mkdir()
            base = {
                "schema": 1,
                "id": "sample",
                "task_kind": "feature",
                "repo_id": "sample",
                "request": "request.md",
                "seed_patch": "seed.patch",
                "reference_patch": "reference.patch",
                "hidden_files": [{"path": "tests/hidden.py", "source": "checks/hidden.py"}],
                "public_commands": [{"id": "public", "argv": ["python3", "-m", "unittest"], "cwd": ".", "timeout_seconds": 10, "expected_output": ["OK"]}],
                "hidden_commands": [{"id": "hidden", "argv": ["python3", "-m", "unittest"], "cwd": ".", "timeout_seconds": 10, "expected_output": ["OK"]}],
                "protected_paths": [".git", "tests/test_public.py", "tests/hidden.py"],
                "generated_roots": [".code-kb"],
                "expected_seed_failure": {"command_id": "hidden", "exit_codes": [1], "diagnostic_contains": "expected"},
                "production_files": ["src/a.py", "src/b.py"],
            }
            (case_dir / "case.json").write_text(json.dumps(base))
            corpus = root / "corpus.json"
            corpus.write_text(json.dumps({"repositories": [{"id": "sample", "url": "https://example.invalid", "commit": "a" * 40}]}))

            base["reference_test_files"] = ["tests/new_public.py"]
            (case_dir / "case.json").write_text(json.dumps(base))
            validated = validate_case(case_dir, corpus_path=corpus, verify_assets=False)
            self.assertEqual(validated["production_files"], ["src/a.py", "src/b.py"])
            self.assertNotIn("tests/new_public.py", validated["protected_paths"])
            self.assertEqual(validated["reference_test_files"], ["tests/new_public.py"])

            for paths, message in (
                (["tests/new_public.py", "tests/new_public.py"], "duplicate paths"),
                (["src/a.py"], "overlaps a production file"),
                (["tests/hidden.py"], "overlaps a hidden destination"),
                (["tests/test_public.py"], "overlaps a protected path"),
                ([".code-kb/generated.py"], "overlaps a generated root"),
                (["tests/new_public", "tests/new_public/test.py"], "reference test files overlap"),
                ([".git/config"], "overlaps protected Git metadata"),
            ):
                with self.subTest(paths=paths):
                    base["reference_test_files"] = paths
                    (case_dir / "case.json").write_text(json.dumps(base))
                    with self.assertRaisesRegex(EvaluationError, message):
                        validate_case(case_dir, corpus_path=corpus, verify_assets=False)

    def test_case_rejects_generated_roots_that_hide_protected_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            case_dir = root / "case"
            case_dir.mkdir()
            case = {
                "schema": 1,
                "id": "sample",
                "task_kind": "feature",
                "repo_id": "sample",
                "request": "request.md",
                "seed_patch": "seed.patch",
                "reference_patch": "reference.patch",
                "hidden_files": [{"path": "tests/hidden.py", "source": "checks/hidden.py"}],
                "public_commands": [{"id": "public", "argv": ["python3", "-m", "unittest"], "cwd": ".", "timeout_seconds": 10, "expected_output": ["OK"]}],
                "hidden_commands": [{"id": "hidden", "argv": ["python3", "-m", "unittest"], "cwd": ".", "timeout_seconds": 10, "expected_output": ["OK"]}],
                "protected_paths": [".git", "tests/test_public.py", "tests/hidden.py"],
                "generated_roots": [".git"],
                "expected_seed_failure": {"command_id": "hidden", "exit_codes": [1], "diagnostic_contains": "expected"},
                "production_files": ["src/a.py", "src/b.py"],
            }
            (case_dir / "case.json").write_text(json.dumps(case))
            for name in ("request.md", "seed.patch", "reference.patch"):
                (case_dir / name).write_text("case data")
            (case_dir / "checks").mkdir()
            (case_dir / "checks/hidden.py").write_text("pass")
            corpus = root / "corpus.json"
            corpus.write_text(json.dumps({"repositories": [{"id": "sample", "url": "https://example.invalid", "commit": "a" * 40}]}))
            with self.assertRaisesRegex(EvaluationError, "generated roots overlap protected paths"):
                validate_case(case_dir, corpus_path=corpus, verify_assets=False)

    def test_case_rejects_parent_traversal_in_commands(self):
        with tempfile.TemporaryDirectory() as temporary:
            case_dir = Path(temporary) / "case"
            case_dir.mkdir()
            case = {
                "schema": 1,
                "id": "sample",
                "task_kind": "feature",
                "repo_id": "sample",
                "request": "request.md",
                "seed_patch": "seed.patch",
                "reference_patch": "reference.patch",
                "hidden_files": [{"path": "tests/hidden.py", "source": "checks/hidden.py"}],
                "public_commands": [{"id": "public", "argv": ["python3", "-m", "unittest"], "cwd": "..", "timeout_seconds": 10}],
                "hidden_commands": [{"id": "hidden", "argv": ["python3", "-m", "unittest"], "cwd": ".", "timeout_seconds": 10}],
                "protected_paths": [".git", "tests/test_public.py", "tests/hidden.py"],
                "generated_roots": [".code-kb"],
                "expected_seed_failure": {"command_id": "hidden", "exit_codes": [1], "diagnostic_contains": "expected"},
                "production_files": ["src/a.py", "src/b.py"],
            }
            for name, content in (("case.json", case), ("request.md", "Task"), ("seed.patch", ""), ("reference.patch", "")):
                path = case_dir / name
                if name == "case.json":
                    path.write_text(json.dumps(content))
                else:
                    path.write_text(content)
            (case_dir / "checks").mkdir()
            (case_dir / "checks/hidden.py").write_text("pass")
            with self.assertRaisesRegex(ValueError, "cwd"):
                validate_case(case_dir, verify_assets=True)

    def test_behavioral_commands_require_literal_output_markers(self):
        with tempfile.TemporaryDirectory() as temporary:
            case_dir = Path(temporary) / "case"
            case_dir.mkdir()
            case = {
                "schema": 1,
                "id": "sample",
                "task_kind": "feature",
                "repo_id": "sample",
                "request": "request.md",
                "seed_patch": "seed.patch",
                "reference_patch": "reference.patch",
                "hidden_files": [{"path": "tests/hidden.py", "source": "checks/hidden.py"}],
                "public_commands": [{"id": "public", "argv": ["python3", "-m", "unittest"], "cwd": ".", "timeout_seconds": 10}],
                "hidden_commands": [{"id": "hidden", "argv": ["python3", "-m", "unittest"], "cwd": ".", "timeout_seconds": 10, "expected_output": ["test_contract"]}],
                "protected_paths": [".git", "tests/hidden.py"],
                "generated_roots": [".code-kb"],
                "expected_seed_failure": {"command_id": "hidden", "exit_codes": [1], "diagnostic_contains": "expected"},
                "production_files": ["src/a.py", "src/b.py"],
            }
            (case_dir / "checks").mkdir()
            (case_dir / "checks/hidden.py").write_text("pass")
            for name, content in (("request.md", "Task"), ("seed.patch", ""), ("reference.patch", "")):
                (case_dir / name).write_text(content)
            (case_dir / "case.json").write_text(json.dumps(case))
            corpus = Path(temporary) / "corpus.json"
            corpus.write_text(json.dumps({"repositories": [{"id": "sample", "url": "https://example.invalid", "commit": "a" * 40}]}))
            with self.assertRaisesRegex(EvaluationError, "expected_output marker"):
                validate_case(case_dir, corpus_path=corpus)
            case["public_commands"][0]["expected_output"] = "OK"
            (case_dir / "case.json").write_text(json.dumps(case))
            with self.assertRaisesRegex(EvaluationError, "expected_output must be a string array"):
                validate_case(case_dir, corpus_path=corpus)


@unittest.skipUnless(shutil.which("bwrap") and Path("/usr/bin/python3").exists(), "requires the Linux execution sandbox")
class EditEvaluationRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="edit-eval-runner-test-")
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        self.source.mkdir()
        (self.source / "src").mkdir()
        (self.source / "tests").mkdir()
        (self.source / "src/__init__.py").write_text("")
        (self.source / "src/a.py").write_text("from .b import value\n\ndef result():\n    return value()\n")
        (self.source / "src/b.py").write_text("def value():\n    return 0\n")
        (self.source / "src/remove.py").write_text("value = 1\n")
        (self.source / "tests/__init__.py").write_text("")
        (self.source / "tests/test_public.py").write_text("import unittest\nfrom pathlib import Path\nfrom src.a import result\n\nclass Public(unittest.TestCase):\n    def test_public(self):\n        self.assertGreaterEqual(result(), 0)\n        self.assertFalse(Path('tests/test_hidden.py').exists())\n")
        (self.source / ".gitignore").write_text("scratch.txt\n")
        self._git("init", "--quiet", "--initial-branch=main")
        self._git("config", "user.name", "fixture")
        self._git("config", "user.email", "fixture@example.invalid")
        self._git("add", "--all")
        self._git("commit", "--quiet", "-m", "pinned fixture")
        self.pin = self._git("rev-parse", "HEAD", output=True)
        self.corpus = self.root / "corpus.json"
        self.corpus.write_text(json.dumps({"schema": 1, "repositories": [{"id": "sample", "url": "https://example.invalid/sample.git", "commit": self.pin}]}))
        self.case_dir = self.root / "case"
        (self.case_dir / "checks").mkdir(parents=True)
        (self.case_dir / "request.md").write_text("Implement the seven-valued result across the calculation and value provider.\n")
        (self.case_dir / "seed.patch").write_text("")
        (self.case_dir / "reference.patch").write_text(
            "diff --git a/src/a.py b/src/a.py\n"
            "--- a/src/a.py\n"
            "+++ b/src/a.py\n"
            "@@ -1,4 +1,4 @@\n"
            " from .b import value\n"
            " \n"
            " def result():\n"
            "-    return value()\n"
            "+    return value() + 1\n"
            "diff --git a/src/b.py b/src/b.py\n"
            "--- a/src/b.py\n"
            "+++ b/src/b.py\n"
            "@@ -1,2 +1,2 @@\n"
            " def value():\n"
            "-    return 0\n"
            "+    return 6\n"
        )
        (self.case_dir / "checks/test_hidden.py").write_text(
            "import unittest\nfrom src.a import result\n\n"
            "class Contract(unittest.TestCase):\n"
            "    def test_contract(self):\n"
            "        self.assertEqual(result(), 7, 'expected value seven')\n"
        )
        command_prefix = ["/usr/bin/env", "PYTHONDONTWRITEBYTECODE=1", "/usr/bin/python3", "-m", "unittest", "-v"]
        case = {
            "schema": 1,
            "id": "sample",
            "task_kind": "feature",
            "repo_id": "sample",
            "request": "request.md",
            "seed_patch": "seed.patch",
            "reference_patch": "reference.patch",
            "hidden_files": [{"path": "tests/test_hidden.py", "source": "checks/test_hidden.py"}],
            "public_commands": [{"id": "public", "argv": [*command_prefix, "tests.test_public"], "cwd": ".", "timeout_seconds": 20, "expected_output": ["test_public"]}],
            "hidden_commands": [{"id": "hidden", "argv": [*command_prefix, "tests.test_hidden"], "cwd": ".", "timeout_seconds": 20, "expected_output": ["test_contract"]}],
            "protected_paths": [".git", ".gitignore", "tests/__init__.py", "tests/test_public.py", "tests/test_hidden.py"],
            "generated_roots": [".code-kb", "__pycache__"],
            "expected_seed_failure": {"command_id": "hidden", "exit_codes": [1], "diagnostic_contains": "expected value seven"},
            "production_files": ["src/a.py", "src/b.py"],
        }
        (self.case_dir / "case.json").write_text(json.dumps(case))
        self.output = self.root / "readiness"

    def tearDown(self):
        self.temporary.cleanup()

    def _git(self, *args, output=False):
        result = subprocess.run(["git", *args], cwd=self.source, check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        return result.stdout.strip() if output else None

    def test_git_commands_preserve_native_home_identities(self):
        environment = _git_env()
        self.assertEqual(environment.get("HOME"), os.environ.get("HOME"))
        self.assertEqual(environment.get("CODEX_HOME"), os.environ.get("CODEX_HOME"))
        self.assertEqual(environment["GIT_CONFIG_GLOBAL"], os.devnull)
        self.assertEqual(environment["GIT_CONFIG_NOSYSTEM"], "1")

    def test_readiness_is_public_green_seed_red_reference_green(self):
        case = json.loads((self.case_dir / "case.json").read_text())
        case["hidden_commands"][0]["expected_output"] = ["OK"]
        (self.case_dir / "case.json").write_text(json.dumps(case))
        cli = subprocess.run(
            [sys.executable, "scripts/edit_evaluation.py", "validate-case", "--case-dir", str(self.case_dir), "--corpus", str(self.corpus)],
            cwd=Path(__file__).resolve().parents[1],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(cli.returncode, 0, cli.stderr)
        readiness = prepare_case_readiness(self.case_dir, self.source, self.output, corpus_path=self.corpus)
        self.assertEqual(readiness["status"], "ready", json.dumps(readiness, sort_keys=True))
        self.assertTrue(readiness["checks"]["baseline_public"]["passed"])
        self.assertTrue(readiness["checks"]["seed_hidden_failure"]["passed"])
        self.assertTrue(readiness["checks"]["reference_public"]["passed"])
        self.assertTrue(readiness["checks"]["reference_hidden"]["passed"])
        self.assertEqual(hash_tree(self.case_dir), readiness["case_sha256"])
        self.assertEqual(readiness["seed_project_path"], "work/seed-project")
        self.assertEqual(readiness["reference_bundle_path"], "evidence/reference-bundle")
        reference_grade = replay_and_grade(
            self.case_dir,
            self.output / readiness["seed_project_path"],
            self.output / readiness["reference_bundle_path"],
            self.root / "reference-grade",
            corpus_path=self.corpus,
        )
        self.assertTrue(reference_grade["behavioral_pass"], reference_grade["checks"])
        self.assertEqual(reference_grade["source_review"], "pending")

    def test_reference_patch_may_add_declared_public_test_but_seed_patch_may_not(self):
        reference_test = "tests/test_reference_public.py"
        reference_addition = (
            "diff --git a/tests/test_reference_public.py b/tests/test_reference_public.py\n"
            "new file mode 100644\n"
            "--- /dev/null\n"
            "+++ b/tests/test_reference_public.py\n"
            "@@ -0,0 +1,5 @@\n"
            "+import unittest\n"
            "+\n"
            "+class ReferencePublic(unittest.TestCase):\n"
            "+    def test_reference_public(self):\n"
            "+        self.assertEqual(1, 1)\n"
        )
        reference_patch = self.case_dir / "reference.patch"
        reference_patch.write_text(reference_patch.read_text() + reference_addition)
        case = json.loads((self.case_dir / "case.json").read_text())
        case["reference_test_files"] = [reference_test]
        (self.case_dir / "case.json").write_text(json.dumps(case))

        readiness = prepare_case_readiness(self.case_dir, self.source, self.output, corpus_path=self.corpus)
        self.assertEqual(readiness["status"], "ready", readiness.get("readiness_error"))
        self.assertFalse((self.output / readiness["seed_project_path"] / reference_test).exists())
        self.assertTrue((self.output / "work/reference-project" / reference_test).is_file())
        bundle = json.loads((self.output / readiness["reference_bundle_path"] / "bundle.json").read_text())
        self.assertIn(reference_test, {change["path"] for change in bundle["changes"]})

        seed_addition = (
            "diff --git a/tests/test_reference_public.py b/tests/test_reference_public.py\n"
            "new file mode 100644\n"
            "--- /dev/null\n"
            "+++ b/tests/test_reference_public.py\n"
            "@@ -0,0 +1 @@\n"
            "+agent-visible replacement\n"
        )
        (self.case_dir / "seed.patch").write_text(seed_addition)
        rejected = prepare_case_readiness(self.case_dir, self.source, self.root / "seed-with-reference-test", corpus_path=self.corpus)
        self.assertEqual(rejected["status"], "not_ready")
        self.assertIn("undeclared paths", rejected["readiness_error"])

    def test_fake_agent_patch_replays_ignored_addition_and_deletion_before_grading(self):
        attempt = self.root / "attempt"
        code = (
            "from pathlib import Path; assert not Path('tests/test_hidden.py').exists(); "
            "Path('src/a.py').write_text('from .b import value\\n\\ndef result():\\n    return value() + 1\\n'); "
            "Path('src/b.py').write_text('def value():\\n    return 6\\n'); "
            "Path('scratch.txt').write_text('ignored addition'); "
            "Path('src/remove.py').unlink()"
        )
        command = [
            sys.executable,
            "scripts/edit_evaluation.py",
            "fake-agent",
            "--case-dir",
            str(self.case_dir),
            "--source-root",
            str(self.source),
            "--corpus",
            str(self.corpus),
            "--attempt-dir",
            str(attempt),
            "--",
            "/usr/bin/python3",
            "-c",
            code,
        ]
        completed = subprocess.run(command, cwd=Path(__file__).resolve().parents[1], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = json.loads(completed.stdout)
        self.assertTrue(result["completed"])
        self.assertTrue(result["patch_valid"])
        self.assertGreaterEqual(result["change_count"], 4)
        self.assertEqual(subprocess.run(["git", "rev-list", "--all", "--count"], cwd=result["seed_project"], check=True, capture_output=True, text=True).stdout.strip(), "1")
        self.assertEqual(subprocess.run(["git", "remote"], cwd=result["seed_project"], check=True, capture_output=True, text=True).stdout.strip(), "")
        grade_cli = subprocess.run(
            [
                sys.executable,
                "scripts/edit_evaluation.py",
                "grade-bundle",
                "--case-dir",
                str(self.case_dir),
                "--seed-project",
                result["seed_project"],
                "--bundle",
                result["bundle_path"],
                "--output",
                str(self.root / "grade"),
                "--corpus",
                str(self.corpus),
            ],
            cwd=Path(__file__).resolve().parents[1],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(grade_cli.returncode, 0, grade_cli.stderr)
        grade = json.loads(grade_cli.stdout)
        self.assertTrue(grade["replay_valid"], grade.get("replay_error"))
        self.assertTrue(grade["behavioral_pass"], grade["checks"])
        self.assertFalse(grade["success"])
        self.assertEqual(grade["source_review"], "pending")
        self.assertEqual(grade["acceptance_status"], "pending_source_review")
        self.assertTrue((attempt / "bundle/blobs").is_dir())

    def test_failing_replay_stays_separate_from_completed_agent_execution(self):
        attempt = self.root / "wrong-attempt"
        result = run_fake_agent(self.case_dir, self.source, attempt, ["/usr/bin/python3", "-c", "from pathlib import Path; Path('src/a.py').write_text('def result(): return 2\\n')"], corpus_path=self.corpus)
        self.assertTrue(result["completed"])
        grade = replay_and_grade(self.case_dir, result["seed_project"], result["bundle_path"], self.root / "wrong-grade", corpus_path=self.corpus)
        self.assertTrue(grade["replay_valid"])
        self.assertFalse(grade["success"])
        self.assertFalse(grade["checks"]["hidden"]["passed"])

    def test_protected_test_write_is_blocked_and_does_not_change_the_seed(self):
        attempt = self.root / "protected-attempt"
        before = (self.source / "tests/test_public.py").read_text()
        result = run_fake_agent(
            self.case_dir,
            self.source,
            attempt,
            ["/usr/bin/python3", "-c", "from pathlib import Path; Path('tests/test_public.py').write_text('broken')"],
            corpus_path=self.corpus,
        )
        self.assertFalse(result["completed"])
        self.assertEqual((self.source / "tests/test_public.py").read_text(), before)
        self.assertEqual(result["protection_status"], "unchanged")
        self.assertTrue(result["protection_denied"])

    def test_invalid_escaping_symlink_is_retained_as_invalid_patch_evidence(self):
        attempt = self.root / "invalid-attempt"
        result = run_fake_agent(
            self.case_dir,
            self.source,
            attempt,
            ["/usr/bin/python3", "-c", "import os; os.symlink('/etc/passwd', 'src/escape')"],
            corpus_path=self.corpus,
        )
        self.assertTrue(result["completed"])
        self.assertFalse(result["patch_valid"])
        self.assertTrue(result["violations"])
        self.assertTrue((attempt / "bundle/bundle.json").is_file())

    def test_timeout_keeps_partial_patch_and_kills_descendant_processes(self):
        attempt = self.root / "timeout-attempt"
        code = (
            "from pathlib import Path; import subprocess,time; "
            "Path('src/a.py').write_text('from .b import value\\n\\ndef result():\\n    return value() + 1\\n'); "
            "Path('src/b.py').write_text('def value():\\n    return 6\\n'); "
            "subprocess.Popen(['/bin/sh','-c','sleep 2; touch /workspace/project/descendant-finished']); "
            "time.sleep(30)"
        )
        result = run_fake_agent(self.case_dir, self.source, attempt, ["/usr/bin/python3", "-c", code], timeout=1, corpus_path=self.corpus)
        self.assertTrue(result["execution"]["timed_out"])
        self.assertFalse(result["completed"])
        self.assertTrue(result["execution"]["process_cleanup"]["complete"])
        changed = {item["path"] for item in json.loads((attempt / "bundle/bundle.json").read_text())["changes"]}
        self.assertIn("src/a.py", changed)
        grade = replay_and_grade(self.case_dir, result["seed_project"], result["bundle_path"], self.root / "timeout-grade", trial_completed=result["completed"], corpus_path=self.corpus)
        self.assertTrue(grade["behavioral_pass"])
        self.assertFalse(grade["success"])
        self.assertEqual(grade["acceptance_status"], "incomplete")
        time.sleep(2.2)
        self.assertFalse((attempt / "agent-project/descendant-finished").exists())

    def test_replay_rejects_a_different_seed_snapshot(self):
        attempt = self.root / "mismatch-attempt"
        result = run_fake_agent(self.case_dir, self.source, attempt, ["/usr/bin/python3", "-c", "pass"], corpus_path=self.corpus)
        wrong_seed = self.root / "wrong-seed"
        shutil.copytree(result["seed_project"], wrong_seed, symlinks=True)
        (wrong_seed / "src/unrelated.py").write_text("value = 10\n")
        grade = replay_and_grade(self.case_dir, wrong_seed, result["bundle_path"], self.root / "mismatch-grade", corpus_path=self.corpus)
        self.assertFalse(grade["replay_valid"])
        self.assertIn("seed identity", grade["replay_error"])
        self.assertFalse(grade["success"])

    def test_malformed_manifest_fails_closed(self):
        manifest = self.root / "manifest.json"
        manifest.write_text(json.dumps({"schema": 1, "status": "ready", "cases": [], "schedule": []}))
        cli = subprocess.run(
            [sys.executable, "scripts/edit_evaluation.py", "validate-manifest", "--manifest", str(manifest), "--corpus", str(self.corpus)],
            cwd=Path(__file__).resolve().parents[1],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(cli.returncode, 2)
        self.assertIn("six pinned repositories", cli.stderr)

    def test_summary_uses_trace_parser_and_keeps_missing_metrics_unknown(self):
        trace = self.root / "trace.jsonl"
        trace.write_text(json.dumps({"type": "turn.completed"}) + "\n")
        summary = summarize_campaign([{
            "case_id": "sample",
            "arm": "native",
            "repeat": 1,
            "completed": True,
            "success": False,
            "input_tokens": None,
            "output_tokens": None,
            "cache_tokens": None,
            "billing": None,
            "trace_path": str(trace),
        }])
        record = summary["records"][0]
        self.assertIsNone(record["input_tokens"])
        self.assertEqual(record["tool_calls"], 0)
        self.assertTrue(record["trace_completed"])
        self.assertIsNone(record["billing"])

    def test_summary_counts_only_accepted_reviewed_completed_valid_behavioral_passes(self):
        pending = {
            "case_id": "pending",
            "success": True,
            "completed": True,
            "patch_valid": True,
            "replay_valid": True,
            "protection_status": "unchanged",
            "behavioral_pass": True,
            "source_review": "pending",
            "acceptance_status": "pending_source_review",
        }
        accepted = {
            "case_id": "accepted",
            "success": False,
            "completed": True,
            "patch_valid": True,
            "replay_valid": True,
            "protection_status": "unchanged",
            "behavioral_pass": True,
            "source_review": "passed",
            "acceptance_status": "accepted",
        }
        rejected = {
            "case_id": "rejected",
            "success": True,
            "completed": True,
            "patch_valid": True,
            "replay_valid": True,
            "protection_status": "unchanged",
            "behavioral_pass": True,
            "source_review": "failed",
            "acceptance_status": "rejected",
        }
        incomplete = {
            "case_id": "incomplete",
            "success": True,
            "completed": False,
            "trial_completed": False,
            "patch_valid": True,
            "replay_valid": True,
            "protection_status": "unchanged",
            "behavioral_pass": True,
            "source_review": "passed",
            "acceptance_status": "accepted",
        }
        invalid = {
            "case_id": "invalid",
            "success": True,
            "completed": True,
            "patch_valid": False,
            "replay_valid": False,
            "protection_status": "unchanged",
            "behavioral_pass": True,
            "source_review": "passed",
            "acceptance_status": "accepted",
        }
        behavior_failed = {
            "case_id": "behavior-failed",
            "success": True,
            "completed": True,
            "patch_valid": True,
            "replay_valid": True,
            "protection_status": "unchanged",
            "behavioral_pass": False,
            "source_review": "passed",
            "acceptance_status": "accepted",
        }
        summary = summarize_campaign([pending, accepted, rejected, incomplete, invalid, behavior_failed])
        rows = {row["case_id"]: row for row in summary["records"]}
        self.assertEqual(summary["success_count"], 1)
        self.assertEqual(summary["accepted_count"], 1)
        self.assertEqual(summary["behavioral_pass_count"], 5)
        self.assertEqual(summary["pending_review_count"], 1)
        self.assertEqual(summary["invalid_patch_count"], 1)
        self.assertEqual(rows["pending"]["acceptance_status"], "pending_source_review")
        self.assertFalse(rows["pending"]["success"])
        self.assertFalse(rows["rejected"]["success"])
        self.assertFalse(rows["incomplete"]["success"])
        self.assertFalse(rows["invalid"]["success"])
        self.assertFalse(rows["behavior-failed"]["success"])

    def test_frozen_manifest_binds_six_ready_cases_controls_and_ordered_cells(self):
        source_corpus = json.loads(DEFAULT_CORPUS.read_text())
        corpus = self.root / "frozen-corpus.json"
        corpus.write_text(json.dumps(source_corpus))
        manifest_root = self.root / "manifest-root"
        cases_root = manifest_root / "cases"
        readiness_root = manifest_root / "readiness"
        artifacts_root = manifest_root / "artifacts"
        for root in (cases_root, readiness_root, artifacts_root):
            root.mkdir(parents=True)

        case_entries = []
        for repo in source_corpus["repositories"]:
            case_dir = cases_root / repo["id"]
            case_dir.mkdir()
            (case_dir / "case.json").write_text(json.dumps({"id": repo["id"]}))
            (case_dir / "reference.patch").write_text("reference")
            case_hash = hash_tree(case_dir)
            readiness_path = readiness_root / f"{repo['id']}.json"
            readiness_path.write_text(json.dumps({"status": "ready", "behavioral_pass": True, "source_review": "passed", "case_id": repo["id"], "source_pin": repo["commit"], "case_sha256": case_hash}))
            case_entries.append({
                "id": repo["id"],
                "repo_id": repo["id"],
                "pin": repo["commit"],
                "case_path": f"cases/{repo['id']}",
                "readiness_path": f"readiness/{repo['id']}.json",
                "case_sha256": case_hash,
                "readiness_sha256": hashlib.sha256(readiness_path.read_bytes()).hexdigest(),
            })

        artifact_names = ("codex", "protocol", "corpus", "snapshot", "sandbox", "runner", "extractor", "binary", "guidance")
        artifacts = {}
        controls = {"model": "gpt-6.1-sol", "effort": "high", "timeout_seconds": 900}
        external_binary = self.root / "external-binary"
        external_binary.write_text("runtime binary bytes\n")
        external_artifacts = {"binary": external_binary}
        source_root = Path(__file__).resolve().parents[1]
        active_sources = {
            "protocol": source_root / "evals/edit-2026-10-27/protocol.md",
            "corpus": corpus,
            "snapshot": source_root / "scripts/edit_snapshot.py",
            "sandbox": source_root / "scripts/edit_sandbox.py",
            "runner": source_root / "scripts/edit_evaluation.py",
        }
        for name in artifact_names:
            artifact = artifacts_root / f"{name}.txt"
            artifact.write_text(corpus.read_text() if name == "corpus" else "logical placeholder\n" if name == "binary" else f"{name} frozen input\n")
            artifacts[name] = f"artifacts/{name}.txt"
            external = active_sources.get(name, external_binary if name == "binary" else artifact)
            external_artifacts[name] = external
            controls[f"{name}_sha256"] = hashlib.sha256(external.read_bytes()).hexdigest()

        schedule = []
        for repeat, arms in ((1, ("native", "code-kb")), (2, ("code-kb", "native"))):
            for case in case_entries:
                for arm in arms:
                    schedule.append({"case_id": case["id"], "arm": arm, "repeat": repeat})
        manifest = {
            "schema": 1,
            "status": "ready",
            "cases": case_entries,
            "schedule": schedule,
            "controls": controls,
            "artifacts": artifacts,
            "call_limit": 28,
            "budget_gate": "pending",
        }
        manifest_path = manifest_root / "manifest.json"
        manifest_path.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(EvaluationError, "missing control artifact paths"):
            validate_manifest(manifest_path, corpus_path=corpus, artifact_paths={"binary": external_binary})

        valid = validate_manifest(manifest_path, corpus_path=corpus, artifact_paths=external_artifacts)
        self.assertTrue(valid["valid"])
        self.assertFalse(valid["dispatch_authorized"])

        cli = subprocess.run(
            [sys.executable, "scripts/edit_evaluation.py", "validate-manifest", "--manifest", str(manifest_path), "--corpus", str(corpus), *[arg for name, value in external_artifacts.items() for arg in ("--artifact", f"{name}={value}")]],
            cwd=source_root,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(cli.returncode, 0, cli.stderr)
        self.assertTrue(json.loads(cli.stdout)["valid"])

        copied_runner = self.root / "copied-runner.py"
        shutil.copyfile(active_sources["runner"], copied_runner)
        wrong_runner_artifacts = dict(external_artifacts, runner=copied_runner)
        with self.assertRaisesRegex(EvaluationError, "active source: runner"):
            validate_manifest(manifest_path, corpus_path=corpus, artifact_paths=wrong_runner_artifacts)

        linked_binary = self.root / "linked-binary"
        linked_binary.symlink_to(external_binary)
        linked_artifacts = dict(external_artifacts, binary=linked_binary)
        with self.assertRaisesRegex(EvaluationError, "symbolic link"):
            validate_manifest(manifest_path, corpus_path=corpus, artifact_paths=linked_artifacts)

        relative_artifacts = dict(external_artifacts, binary=Path("external-binary"))
        with self.assertRaisesRegex(EvaluationError, "absolute path"):
            validate_manifest(manifest_path, corpus_path=corpus, artifact_paths=relative_artifacts)

        manifest["schedule"].reverse()
        manifest_path.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(EvaluationError, "schedule"):
            validate_manifest(manifest_path, corpus_path=corpus, artifact_paths=external_artifacts)
        manifest["schedule"].reverse()
        manifest_path.write_text(json.dumps(manifest))
        external_binary.write_text("stale runtime binary\n")
        with self.assertRaisesRegex(EvaluationError, "stale: binary"):
            validate_manifest(manifest_path, corpus_path=corpus, artifact_paths=external_artifacts)


if __name__ == "__main__":
    unittest.main()


if __name__ == "__main__":
    unittest.main()
