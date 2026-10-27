import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

from scripts.edit_sandbox import SandboxError, _codex_arguments


SCRIPT = Path(__file__).with_name("edit_sandbox.py")
CODEX_SHIM = Path(shutil.which("codex") or "/nonexistent").resolve()
CODEX_NATIVE = CODEX_SHIM.parent.parent / "node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex"


@unittest.skipUnless(sys.platform.startswith("linux") and shutil.which("bwrap"), "Linux with bubblewrap is required")
class EditSandboxTest(unittest.TestCase):
    def invoke(self, project, state, output, mode, argv, *, options=(), timeout="10", env=None):
        command = [
            sys.executable,
            str(SCRIPT),
            "run",
            "--project",
            str(project),
            "--state",
            str(state),
            "--output",
            str(output),
            "--timeout",
            str(timeout),
            "--mode",
            mode,
            *options,
            "--",
            *argv,
        ]
        return subprocess.run(command, capture_output=True, text=True, env=env)

    def test_agent_command_rejects_inline_sandbox_overrides(self):
        for option in (
            "--profile=unrestricted",
            "--permission-profile=unrestricted",
            "--sandbox=danger-full-access",
            "--add-dir=/home/murphy",
            "--config=sandbox_mode='danger-full-access'",
            "--profile=unrestricted",
        ):
            with self.subTest(option=option), self.assertRaises(SandboxError):
                _codex_arguments(["codex", "exec", option, "prompt"])
        for option in ("--profile=unrestricted", "--cd=/workspace/other"):
            with self.subTest(option=option), self.assertRaises(SandboxError):
                _codex_arguments(["codex", "exec", "prompt", option])
        with self.assertRaises(SandboxError):
            _codex_arguments(
                ["codex", "sandbox", "--cd", "/workspace/project", "--cd=/tmp", "--", "true"]
            )
        for option in (
            "-cpermissions.edit_evaluation.network.enabled=true",
            "-pdifferent",
            "-Punrestricted",
            "-sdanger-full-access",
        ):
            for command in ("exec", "sandbox"):
                argv = ["codex", command, option, "prompt"]
                if command == "sandbox":
                    argv.extend(("--", "true"))
                with self.subTest(command=command, option=option), self.assertRaises(SandboxError):
                    _codex_arguments(argv)
            with self.subTest(command="exec-late", option=option), self.assertRaises(SandboxError):
                _codex_arguments(["codex", "exec", "prompt", option])

    def test_run_cli_edits_project_and_reads_read_only_dependency(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            state = base / "state"
            output = base / "logs"
            dependency = base / "dependency"
            project.mkdir()
            dependency.mkdir()
            (project / "source.txt").write_text("before")
            (dependency / "input.txt").write_text("prepared")
            command = [
                sys.executable,
                str(SCRIPT),
                "run",
                "--project",
                str(project),
                "--state",
                str(state),
                "--output",
                str(output),
                "--timeout",
                "10",
                "--mode",
                "grade",
                "--dependency",
                f"reviewed={dependency}",
                "--",
                "python3",
                "-c",
                "from pathlib import Path; source=Path('/deps/reviewed/input.txt'); "
                "Path('/workspace/project/source.txt').write_text(source.read_text()); "
                "Path('/state/cache').mkdir(); Path('/state/cache/lock').write_text('disposable'); "
                "exec(\"try:\\n source.write_text('changed')\\n dependency_writable=True\\nexcept OSError:\\n dependency_writable=False\"); "
                "print('dependency_writable=' + str(dependency_writable))",
            ]
            result = subprocess.run(command, capture_output=True, text=True)

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((project / "source.txt").read_text(), "prepared")
            self.assertEqual((dependency / "input.txt").read_text(), "prepared")
            self.assertEqual((state / "cache" / "lock").read_text(), "disposable")
            self.assertIn("dependency_writable=False", Path(json.loads(result.stdout)["stdout_log"]).read_text())
            report = json.loads(result.stdout)
            self.assertEqual(report["exit_status"], 0)
            self.assertFalse(report["timed_out"])
            self.assertTrue(Path(report["stdout_log"]).is_file())
            self.assertTrue(Path(report["stderr_log"]).is_file())

    def test_protected_paths_host_files_and_logs_are_outside_child_mounts(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            (project / "tests" / "nested").mkdir(parents=True)
            (project / "source.txt").write_text("before")
            protected = project / "tests" / "test_case.py"
            protected.write_text("canonical")
            nested_protected = project / "tests" / "nested" / "canonical.py"
            nested_protected.write_text("nested-canonical")
            canary = base / "host-canary.txt"
            canary.write_text("host-secret")
            output = base / "logs"
            code = f"""import json
from pathlib import Path
result = {{}}
Path('/workspace/project/source.txt').write_text('edited')
protected = Path('/workspace/project/tests/test_case.py')
try:
    protected.write_text('weakened')
    result['protected_write'] = True
except OSError:
    result['protected_write'] = False
try:
    Path('/workspace/project/tests').rename('/workspace/project/tests-renamed')
    result['protected_parent_rename'] = True
except OSError:
    result['protected_parent_rename'] = False
try:
    Path('/workspace/project/tests/nested').rename('/workspace/project/tests/nested-renamed')
    result['nested_parent_rename'] = True
except OSError:
    result['nested_parent_rename'] = False
Path('/workspace/project/tests/new_test.py').write_text('new test')
canary = Path({str(canary)!r})
try:
    result['host_canary'] = canary.read_text() == 'host-secret'
except OSError:
    result['host_canary'] = False
log = Path({str(output / 'injected.log')!r})
try:
    log.write_text('injected')
    result['log_write'] = True
except OSError:
    result['log_write'] = False
print(json.dumps(result))"""
            result = self.invoke(
                project,
                base / "state",
                output,
                "grade",
                ["python3", "-c", code],
                options=("--protect", "tests/test_case.py", "--protect", "tests/nested/canonical.py"),
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads(result.stdout)
            child_result = json.loads(Path(report["stdout_log"]).read_text())
            self.assertEqual((project / "source.txt").read_text(), "edited")
            self.assertEqual(protected.read_text(), "canonical")
            self.assertFalse(child_result["protected_write"])
            self.assertFalse(child_result["protected_parent_rename"])
            self.assertFalse(child_result["nested_parent_rename"])
            self.assertTrue((project / "tests" / "test_case.py").is_file())
            self.assertFalse((project / "tests-renamed").exists())
            self.assertEqual(nested_protected.read_text(), "nested-canonical")
            self.assertFalse((project / "tests" / "nested-renamed").exists())
            self.assertEqual((project / "tests" / "new_test.py").read_text(), "new test")
            self.assertFalse(child_result["host_canary"])
            self.assertFalse(child_result["log_write"])
            self.assertFalse((output / "injected.log").exists())

    def test_grade_drops_host_environment_and_network_access(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            server = socket.socket()
            server.bind(("127.0.0.1", 0))
            server.listen(1)
            server.settimeout(0.2)
            port = server.getsockname()[1]
            host_environment = os.environ.copy()
            host_environment["EVAL_HOST_SECRET"] = "sentinel-value"
            host_environment["OPENAI_API_KEY"] = "not-a-real-key"
            code = (
                "import json, os, socket; result = {}; "
                "result['secret_environment'] = any(k in os.environ for k in ('EVAL_HOST_SECRET', 'OPENAI_API_KEY')); "
                f"result['home_identity'] = os.environ.get('HOME') == {os.environ.get('HOME')!r}; "
                f"result['codex_identity'] = os.environ.get('CODEX_HOME') == {os.environ.get('CODEX_HOME')!r}; "
                "connection = socket.socket(); connection.settimeout(1); "
                "result['host_network'] = connection.connect_ex(('127.0.0.1', "
                f"{port})) == 0; connection.close(); print(json.dumps(result))"
            )
            try:
                result = self.invoke(
                    project,
                    base / "state",
                    base / "logs",
                    "grade",
                    ["python3", "-c", code],
                    env=host_environment,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                report = json.loads(result.stdout)
                child_result = json.loads(Path(report["stdout_log"]).read_text())
                self.assertFalse(child_result["secret_environment"])
                self.assertTrue(child_result["home_identity"])
                self.assertTrue(child_result["codex_identity"])
                self.assertFalse(child_result["host_network"])
                with self.assertRaises(TimeoutError):
                    server.accept()
            finally:
                server.close()

    def test_preparation_network_is_available_only_when_explicit(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            server = socket.socket()
            server.bind(("127.0.0.1", 0))
            server.listen(1)
            server.settimeout(2)
            port = server.getsockname()[1]
            code = (
                "import json, socket; connection = socket.socket(); connection.settimeout(1); "
                f"success = connection.connect_ex(('127.0.0.1', {port})) == 0; "
                "connection.close(); print(json.dumps({'connected': success}))"
            )
            try:
                result = self.invoke(
                    project,
                    base / "state",
                    base / "logs",
                    "prepare",
                    ["python3", "-c", code],
                    options=("--network",),
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                report = json.loads(result.stdout)
                child_result = json.loads(Path(report["stdout_log"]).read_text())
                self.assertTrue(child_result["connected"])
                accepted, _ = server.accept()
                accepted.close()
            finally:
                server.close()

    def test_timeout_kills_descendants_and_keeps_partial_project_edits(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            code = (
                "import pathlib, subprocess, sys, time; "
                "pathlib.Path('/workspace/project/partial.txt').write_text('kept'); "
                "subprocess.Popen([sys.executable, '-c', \"import pathlib, time; time.sleep(1.5); pathlib.Path('/workspace/project/late.txt').write_text('late')\"]); "
                "print('started-child', flush=True); time.sleep(30)"
            )
            result = self.invoke(
                project,
                base / "state",
                base / "logs",
                "grade",
                ["python3", "-c", code],
                timeout="0.4",
            )

            self.assertEqual(result.returncode, 124)
            report = json.loads(result.stdout)
            self.assertTrue(report["timed_out"])
            self.assertTrue(report["process_cleanup"]["complete"])
            self.assertEqual((project / "partial.txt").read_text(), "kept")
            self.assertIn("started-child", Path(report["stdout_log"]).read_text())
            time.sleep(1.7)
            self.assertFalse((project / "late.txt").exists())

    def test_timeout_kills_descendants_that_ignore_termination(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            child = (
                "import pathlib, signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
                "time.sleep(1.5); pathlib.Path('/workspace/project/late.txt').write_text('late')"
            )
            code = (
                "import subprocess, sys, time; "
                f"subprocess.Popen([sys.executable, '-c', {child!r}]); time.sleep(30)"
            )
            result = self.invoke(
                project,
                base / "state",
                base / "logs",
                "grade",
                ["python3", "-c", code],
                timeout="0.4",
            )

            self.assertEqual(result.returncode, 124)
            self.assertTrue(json.loads(result.stdout)["process_cleanup"]["complete"])
            time.sleep(1.7)
            self.assertFalse((project / "late.txt").exists())

    def test_invalid_protection_and_grade_network_option_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            state = base / "state"
            outside = self.invoke(
                project,
                state,
                base / "logs-traversal",
                "grade",
                ["python3", "-c", "raise SystemExit(99)"],
                options=("--protect", "../host-secret"),
            )
            network = self.invoke(
                project,
                base / "network-state",
                base / "logs-network",
                "grade",
                ["python3", "-c", "raise SystemExit(99)"],
                options=("--network",),
            )

            self.assertEqual(outside.returncode, 2)
            self.assertIn("relative path", outside.stderr)
            self.assertEqual(network.returncode, 2)
            self.assertIn("only in preparation", network.stderr)
            self.assertFalse((base / "logs-traversal").exists())
            self.assertFalse((base / "logs-network").exists())
            self.assertFalse(state.exists())

    def test_auth_options_are_agent_only_and_modes_are_validated_before_mounts(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            profile = base / "private-profile"
            profile.mkdir()
            auth = base / "auth.json"
            auth.write_text("{}")
            auth.chmod(0o600)
            for mode in ("prepare", "grade"):
                state = base / f"{mode}-state"
                output = base / f"{mode}-logs"
                result = self.invoke(
                    project,
                    state,
                    output,
                    mode,
                    ["python3", "-c", "raise SystemExit(99)"],
                    options=("--private-profile", str(profile), "--auth-file", str(auth)),
                )

                self.assertEqual(result.returncode, 2)
                self.assertIn("only in agent mode", result.stderr)
                self.assertFalse(state.exists())
                self.assertFalse(output.exists())
            unsupported = self.invoke(
                project,
                base / "unsupported-state",
                base / "unsupported-logs",
                "unknown",
                ["python3", "-c", "raise SystemExit(99)"],
            )

            self.assertEqual(unsupported.returncode, 2)
            self.assertFalse((base / "unsupported-state").exists())
            self.assertFalse((base / "unsupported-logs").exists())

    def test_broad_or_private_tool_inputs_cannot_be_mounted(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            broad = self.invoke(
                project,
                base / "state-broad",
                base / "logs-broad",
                "grade",
                ["python3", "-c", "raise SystemExit(99)"],
                options=("--dependency", f"home={Path.home()}"),
            )
            private = self.invoke(
                project,
                base / "state-private",
                base / "logs-private",
                "grade",
                ["python3", "-c", "raise SystemExit(99)"],
                options=("--tool", f"private={Path.home() / '.codex'}"),
            )

            self.assertEqual(broad.returncode, 2)
            self.assertIn("broad directory", broad.stderr)
            self.assertEqual(private.returncode, 2)
            self.assertIn("private host path", private.stderr)
            self.assertFalse((base / "logs-broad").exists())
            self.assertFalse((base / "logs-private").exists())

    def test_state_cannot_alias_a_native_private_profile_path(self):
        with tempfile.TemporaryDirectory(dir=Path.home()) as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            home = base / "home"
            code_home = home / ".codex"
            home.mkdir()
            code_home.mkdir()
            state = code_home / "sandbox-state"
            result = self.invoke(
                project,
                state,
                base / "logs",
                "grade",
                ["python3", "-c", "raise SystemExit(99)"],
                env={**os.environ, "HOME": str(home), "CODEX_HOME": str(code_home)},
            )

            self.assertEqual(result.returncode, 2)
            self.assertIn("private host path", result.stderr)
            self.assertFalse(state.exists())
            self.assertFalse((base / "logs").exists())

    def test_project_and_state_are_allowed_inside_codex_worktrees(self):
        with tempfile.TemporaryDirectory(dir=Path.home()) as temporary:
            base = Path(temporary)
            home = base / "identity"
            code_home = home / ".codex"
            worktree = code_home / "worktrees" / "task"
            project = worktree / "source"
            state = worktree / "target" / "state"
            project.mkdir(parents=True)
            state.parent.mkdir()
            home.mkdir(exist_ok=True)
            env = {**os.environ, "HOME": str(home), "CODEX_HOME": str(code_home)}
            result = self.invoke(
                project,
                state,
                base / "logs",
                "grade",
                ["python3", "-c", "from pathlib import Path; Path('/workspace/project/edited.txt').write_text('ok')"],
                env=env,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((project / "edited.txt").read_text(), "ok")
            self.assertTrue(state.is_dir())

    @unittest.skipUnless(CODEX_NATIVE.is_file(), "installed native Codex CLI is required")
    def test_agent_rejects_legacy_permission_config_before_creating_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            profile = base / "private-profile"
            profile.mkdir()
            (profile / "config.toml").write_text('sandbox_mode = "danger-full-access"\n')
            state = base / "state"
            output = base / "logs"
            result = self.invoke(
                project,
                state,
                output,
                "agent",
                ["codex", "sandbox", "--", "python3", "-c", "raise SystemExit(99)"],
                options=("--tool", f"codex={CODEX_NATIVE}", "--private-profile", str(profile)),
            )

            self.assertEqual(result.returncode, 2)
            self.assertIn("legacy sandbox settings", result.stderr)
            self.assertFalse(state.exists())
            self.assertFalse(output.exists())

    @unittest.skipUnless(CODEX_NATIVE.is_file(), "installed native Codex CLI is required")
    def test_agent_profile_denies_private_reads_writes_and_command_network(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            profile = base / "private-profile"
            profile.mkdir()
            (profile / "private-canary.txt").write_text("private-profile-content")
            auth = base / "auth.json"
            auth.write_text('{"test_auth":"sentinel"}')
            auth.chmod(0o600)
            server = socket.socket()
            server.bind(("127.0.0.1", 0))
            server.listen(1)
            server.settimeout(0.2)
            port = server.getsockname()[1]
            code = f"""import json, os, pathlib, socket
profile = pathlib.Path(os.environ.get('CODEX_HOME', str(pathlib.Path.home() / '.codex')))
result = {{}}
try:
    result['private_read'] = (profile / 'private-canary.txt').read_text() == 'private-profile-content'
except OSError:
    result['private_read'] = False
try:
    result['state_alias_read'] = any(path.read_text() == 'private-profile-content' for path in pathlib.Path('/state').rglob('private-canary.txt'))
except OSError:
    result['state_alias_read'] = False
try:
    result['auth_read'] = (profile / 'auth.json').read_text() == '{{"test_auth":"sentinel"}}'
except OSError:
    result['auth_read'] = False
try:
    (profile / 'private-canary.txt').write_text('changed')
    result['private_write'] = True
except OSError:
    result['private_write'] = False
pathlib.Path('/workspace/project/agent-edit.txt').write_text('permitted')
try:
    connection = socket.socket()
    connection.settimeout(1)
    result['network'] = connection.connect_ex(('127.0.0.1', {port})) == 0
    connection.close()
except OSError:
    result['network'] = False
print(json.dumps(result))"""
            result = self.invoke(
                project,
                base / "state",
                base / "logs",
                "agent",
                ["codex", "sandbox", "--cd", "/workspace/project", "--", "python3", "-c", code],
                options=("--tool", f"codex={CODEX_NATIVE}", "--private-profile", str(profile), "--auth-file", str(auth)),
                timeout="30",
            )

            try:
                detail = result.stdout + result.stderr
                if result.stdout:
                    detail += Path(json.loads(result.stdout)["stderr_log"]).read_text()
                self.assertEqual(result.returncode, 0, detail)
                report = json.loads(result.stdout)
                child_result = json.loads(Path(report["stdout_log"]).read_text())
                self.assertFalse(child_result["private_read"])
                self.assertFalse(child_result["state_alias_read"])
                self.assertFalse(child_result["auth_read"])
                self.assertFalse(child_result["private_write"])
                self.assertFalse(child_result["network"])
                self.assertEqual((project / "agent-edit.txt").read_text(), "permitted")
                with self.assertRaises(TimeoutError):
                    server.accept()
            finally:
                server.close()

    @unittest.skipUnless(CODEX_NATIVE.is_file(), "installed native Codex CLI is required")
    def test_agent_profile_selftest_needs_no_private_profile_or_auth(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            project = base / "project"
            project.mkdir()
            result = self.invoke(
                project,
                base / "state",
                base / "logs",
                "agent",
                [
                    "codex",
                    "sandbox",
                    "--",
                    "python3",
                    "-c",
                    "from pathlib import Path; Path('/workspace/project/selftest.txt').write_text('sandboxed'); "
                    "Path('/state/tool-cache').mkdir(); Path('/state/tool-cache/probe').write_text('sandboxed')",
                ],
                options=("--tool", f"codex={CODEX_NATIVE}"),
                timeout="30",
            )

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            report = json.loads(result.stdout)
            self.assertEqual(report["exit_status"], 0)
            self.assertEqual((project / "selftest.txt").read_text(), "sandboxed")
            self.assertEqual((base / "state" / "tool-cache" / "probe").read_text(), "sandboxed")


if __name__ == "__main__":
    unittest.main()
