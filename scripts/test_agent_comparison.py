import unittest
import tempfile
from pathlib import Path
import subprocess
import json
import sys
import shutil
import os
from unittest.mock import patch

import agent_comparison


class ScheduleTests(unittest.TestCase):
    def test_each_task_places_each_arm_in_each_position_once(self):
        self.assertTrue(hasattr(agent_comparison, "schedule"), "balanced scheduler is missing")
        rows = agent_comparison.schedule(["first", "second"])
        self.assertEqual(len(rows), 18)
        self.assertEqual(
            [(row["repeat"], row["order"], row["arm"]) for row in rows[:9]],
            [
                (1, 1, "native"), (1, 2, "current"), (1, 3, "revised"),
                (2, 1, "revised"), (2, 2, "native"), (2, 3, "current"),
                (3, 1, "current"), (3, 2, "revised"), (3, 3, "native"),
            ],
        )
        self.assertEqual({row["task_id"] for row in rows[:9]}, {"first"})
        self.assertEqual({row["task_id"] for row in rows[9:]}, {"second"})


class CaptureTests(unittest.TestCase):
    def test_capture_does_not_inherit_parent_agent_session(self):
        with tempfile.TemporaryDirectory() as name, patch.dict(os.environ, {"CODEX_THREAD_ID": "parent-thread", "CODEX_SESSION_ID": "parent-session", "CODEX_TOOL_CALL_ID": "parent-tool"}):
            output = Path(name)
            agent_comparison.capture(
                [sys.executable, "-c", "import os,json; print(json.dumps([os.environ.get(k) for k in ('CODEX_THREAD_ID','CODEX_SESSION_ID','CODEX_TOOL_CALL_ID')]))"],
                "synthetic prompt", output, timeout=3,
            )
            self.assertEqual(json.loads((output / "events.jsonl").read_text()), [None, None, None])

    def test_capture_preserves_jsonl_stderr_and_the_actual_exit_status(self):
        self.assertTrue(hasattr(agent_comparison, "capture"), "bounded transcript capture is missing")
        with tempfile.TemporaryDirectory() as name:
            output = Path(name)
            result = agent_comparison.capture(
                [sys.executable, "-c", "import sys; print('{\"type\":\"turn.failed\"}'); print('transport unavailable',file=sys.stderr); sys.exit(7)"],
                "synthetic prompt", output, timeout=3,
            )
            self.assertEqual(result["exit_code"], 7)
            self.assertFalse(result["timed_out"])
            self.assertEqual(json.loads((output / "events.jsonl").read_text()), {"type": "turn.failed"})
            self.assertIn("transport unavailable", (output / "stderr.log").read_text())

    def test_capture_terminates_a_stalled_process_at_its_deadline(self):
        self.assertTrue(hasattr(agent_comparison, "capture"), "bounded transcript capture is missing")
        with tempfile.TemporaryDirectory() as name:
            result = agent_comparison.capture(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                "synthetic prompt", Path(name), timeout=0.05,
            )
            self.assertTrue(result["timed_out"])
            self.assertLess(result["wall_seconds"], 2)


class ConfigurationTests(unittest.TestCase):
    @unittest.skipUnless(
        shutil.which("codex") and shutil.which("bwrap") and (Path.home() / ".codex").is_dir(),
        "needs codex, bubblewrap, and a Codex profile",
    )
    def test_codex_sandbox_cannot_read_private_profile(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            project = root / "project"
            project.mkdir()
            profile = root / "profile"
            profile.mkdir()
            (profile / "probe.txt").write_text("private sentinel")
            authentication = root / "fake-auth.json"
            authentication.write_text("{}")
            target = Path.home() / ".codex"
            configured = agent_comparison.codex_command(
                shutil.which("codex"), project, root / "schema", root / "answer", target, "probe",
            )
            overrides = [value for index in range(len(configured) - 1) if configured[index] == "-c" for value in configured[index:index + 2]]
            argv = [configured[0], "sandbox", "-P", "comparison", "-C", str(project), *overrides, "--", "/usr/bin/python3", "-c", "from pathlib import Path; import sys; print('started',flush=True); Path(sys.argv[1]).read_text()", str(target / "probe.txt")]
            command = agent_comparison.isolate(argv, root, profile, target, authentication, [])
            result = subprocess.run(command, capture_output=True, text=True, timeout=20)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("started", result.stdout, result.stderr)
            self.assertIn("PermissionError", result.stderr)

    def test_native_configuration_excludes_mcp_and_protects_private_profile(self):
        self.assertTrue(hasattr(agent_comparison, "codex_command"), "isolated Codex configuration is missing")
        command = agent_comparison.codex_command(
            "/bin/codex", Path("/tmp/project"), Path("/tmp/schema.json"),
            Path("/tmp/answer.json"), Path("/home/eval/.codex"), "native policy",
        )
        for flag in ("--ignore-user-config", "--ignore-rules", "--ephemeral", "--json", "--strict-config"):
            self.assertIn(flag, command)
        values = [command[index + 1] for index, value in enumerate(command[:-1]) if value == "-c"]
        self.assertFalse(any(value.startswith("mcp_servers.") for value in values))
        self.assertIn('permissions.comparison.filesystem={"/home/eval/.codex" = "deny"}', values)
        self.assertIn('default_permissions="comparison"', values)
        self.assertNotIn("--sandbox", command)

    def test_mcp_configuration_binds_only_candidate_and_current_project(self):
        self.assertTrue(hasattr(agent_comparison, "codex_command"), "isolated Codex configuration is missing")
        command = agent_comparison.codex_command(
            "/bin/codex", Path("/tmp/project"), Path("/tmp/schema.json"),
            Path("/tmp/answer.json"), Path("/home/eval/.codex"), "frozen policy",
            binary=Path("/tmp/bin/code-kb"), telemetry=Path("/tmp/telemetry"),
        )
        values = [command[index + 1] for index, value in enumerate(command[:-1]) if value == "-c"]
        self.assertIn('mcp_servers.code_kb.command="/tmp/bin/code-kb"', values)
        self.assertIn('mcp_servers.code_kb.args=["serve", "--root", "/tmp/project"]', values)
        self.assertIn('mcp_servers.code_kb.env.JULIE_EXTRACT_BIN="/tmp/bin/julie-extract"', values)
        approvals = {value for value in values if ".approval_mode=" in value}
        self.assertEqual(approvals, {
            'mcp_servers.code_kb.tools.file_skeleton.approval_mode="approve"',
            'mcp_servers.code_kb.tools.lookup_symbol.approval_mode="approve"',
            'mcp_servers.code_kb.tools.get_symbol_body.approval_mode="approve"',
            'mcp_servers.code_kb.tools.get_symbol_context.approval_mode="approve"',
            'mcp_servers.code_kb.tools.find_references.approval_mode="approve"',
            'mcp_servers.code_kb.tools.blast_radius.approval_mode="approve"',
            'mcp_servers.code_kb.tools.check_edit.approval_mode="approve"',
        })
        self.assertFalse(any("goldfish" in value for value in values))


class IsolationTests(unittest.TestCase):
    def test_staging_refuses_a_root_containing_an_answer_key(self):
        self.assertTrue(hasattr(agent_comparison, "stage_fixture"), "fixture isolation is missing")
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            fixture = root / "fixture"
            fixture.mkdir()
            (fixture / "oracles.json").write_text("hidden answer")
            with self.assertRaises(ValueError):
                agent_comparison.stage_fixture(fixture, root / "project")

    def test_staging_copies_only_fixture_and_omits_generated_state(self):
        self.assertTrue(hasattr(agent_comparison, "stage_fixture"), "fixture isolation is missing")
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            fixture = root / "fixture"
            (fixture / "target").mkdir(parents=True)
            (fixture / "target" / "answer.txt").write_text("old output")
            (fixture / "sample.py").write_text("def probe(): return 1\n")
            (root / "oracles.json").write_text("hidden answer")
            destination = root / "project"
            agent_comparison.stage_fixture(fixture, destination)
            self.assertEqual((destination / "sample.py").read_text(), "def probe(): return 1\n")
            self.assertFalse((destination / "oracles.json").exists())
            self.assertFalse((destination / "target").exists())

    def test_staging_rejects_a_symlink_to_an_answer(self):
        self.assertTrue(hasattr(agent_comparison, "stage_fixture"), "fixture isolation is missing")
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            fixture = root / "fixture"
            fixture.mkdir()
            (root / "answer.json").write_text("hidden answer")
            (fixture / "sample.py").symlink_to(root / "answer.json")
            with self.assertRaises(ValueError):
                agent_comparison.stage_fixture(fixture, root / "project")

    def test_namespace_hides_host_answers_and_host_instructions(self):
        self.assertTrue(hasattr(agent_comparison, "isolate"), "filesystem isolation is missing")
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            project = root / "project"
            project.mkdir()
            (project / "probe.txt").write_text("visible")
            hidden = root / "owner"
            hidden.mkdir()
            (hidden / "oracles.json").write_text("hidden answer")
            host_profile = root / "codex"
            host_profile.mkdir()
            (host_profile / "AGENTS.md").write_text("host instructions")
            (host_profile / "auth.json").write_text("fake calibration credential")
            private_profile = root / "private"
            private_profile.mkdir()
            argv = agent_comparison.isolate(
                ["/usr/bin/python3", "-c", "from pathlib import Path; import sys; assert Path.cwd() == Path(sys.argv[3]).parent; assert not Path(sys.argv[1]).exists(); assert not Path(sys.argv[2]).exists(); print(Path(sys.argv[3]).read_text())", str(hidden / "oracles.json"), str(host_profile / "AGENTS.md"), str(project / "probe.txt")],
                root, private_profile, host_profile, host_profile / "auth.json", [hidden],
            )
            result = subprocess.run(argv, text=True, capture_output=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), "visible")
