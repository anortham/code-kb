import csv
import hashlib
import json
import sqlite3
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from scripts import comparison_campaign as campaign


ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "scripts" / "comparison_campaign.py"


class CampaignCommandTests(unittest.TestCase):
    def make_manifest_project(self, directory):
        manifest = json.loads((ROOT / "evals/review-2026-10-26/experiment.json").read_text())
        project = Path(directory) / "project"
        frozen_paths = set(manifest["files"]) | {
            "scripts/agent_comparison.py",
            "scripts/comparison_metrics.py",
            "scripts/comparison_campaign.py",
        }
        manifest["files"] = {}
        for relative in frozen_paths:
            source = ROOT / relative
            destination = project / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
            manifest["files"][relative] = hashlib.sha256(destination.read_bytes()).hexdigest()
        binary = Path(directory) / "code-kb"
        julie = Path(directory) / "julie-extract"
        binary.write_bytes(b"test engine")
        julie.write_bytes(b"test extractor")
        manifest["engine_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        manifest["extractor_sha256"] = hashlib.sha256(julie.read_bytes()).hexdigest()
        manifest_path = project / "evals/review-2026-10-26/experiment.json"
        manifest_path.write_text(json.dumps(manifest))
        output = Path(directory) / "output"
        output.mkdir()
        return project, output, binary, julie

    def campaign_args(self, project, output, binary, julie):
        return SimpleNamespace(run=True, root=project, output=output, binary=binary, julie=julie, codex=Path("/bin/false"))

    def test_campaign_dispatches_73_jobs_without_authorization_marker(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            jobs = [
                {"task_id": "python-registration-concept", "repeat": index + 1, "order": index + 1, "arm": "native"}
                for index in range(73)
            ]
            authorization_marker_exists = (output / "authorized.json").exists()
            calls = []

            def completed_call(argv, prompt, capture_dir, timeout=360):
                calls.append(argv)
                capture_dir = Path(capture_dir)
                capture_dir.mkdir(parents=True, exist_ok=True)
                (capture_dir / "events.jsonl").write_text('{"type":"turn.completed"}\n')
                (capture_dir / "stderr.log").write_text("")
                answer = Path(argv[argv.index("-o") + 1])
                answer.write_text(json.dumps({"symbols": [], "tests": [], "literals": [], "explanation": "No facts supplied."}))
                return {"exit_code": 0, "timed_out": False, "wall_seconds": 0.25}

            try:
                result = campaign.run_campaign(
                    self.campaign_args(project, output, binary, julie),
                    jobs=jobs,
                    auth_file=auth_file,
                    hidden_roots=[],
                    capture_fn=completed_call,
                    redactor=lambda payload: payload,
                )
            except campaign.CampaignError as error:
                self.fail(f"a valid campaign was blocked before fake dispatch: {error}")
            outcomes = [json.loads(path.read_text()) for path in (output / "runs").glob("*/outcome.json")]

        self.assertFalse(authorization_marker_exists)
        self.assertEqual(result["status"], "complete")
        self.assertEqual(len(calls), 73)
        self.assertEqual(len(outcomes), 73)
        self.assertEqual({outcome["infrastructure_retries_used"] for outcome in outcomes}, {0})

    def test_fixture_drift_fails_before_campaign_dispatch(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            fixture = project / "evals/review-2026-10-26/fixture/python/app.py"
            fixture.write_text(fixture.read_text() + "\n# changed after freeze\n")
            completed = subprocess.run(
                [
                    sys.executable,
                    str(RUNNER),
                    "--root",
                    str(project),
                    "--output",
                    str(output),
                    "--binary",
                    str(binary),
                    "--julie",
                    str(julie),
                    "--codex",
                    "/bin/false",
                    "--run",
                ],
                capture_output=True,
                text=True,
                check=False,
            )

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("manifest", completed.stderr.lower())

    def test_resuming_preserves_and_reports_an_infrastructure_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 1, "arm": "native"}
            attempts = []

            def cannot_start(argv, prompt, capture_dir, timeout=360):
                attempts.append(argv)
                raise OSError("codex could not start")

            first = campaign.run_campaign(
                self.campaign_args(project, output, binary, julie), jobs=[job], auth_file=auth_file,
                hidden_roots=[], capture_fn=cannot_start, redactor=lambda payload: payload,
            )
            outcome_path = output / "runs/python-registration-concept-native-r1/outcome.json"
            before = outcome_path.read_bytes()
            second = campaign.run_campaign(
                self.campaign_args(project, output, binary, julie), jobs=[job], auth_file=auth_file,
                hidden_roots=[], capture_fn=cannot_start, redactor=lambda payload: payload,
            )
            after = outcome_path.read_bytes()

        self.assertEqual(first["status"], "infrastructure_error")
        self.assertEqual(second["status"], "incomplete")
        self.assertEqual(len(attempts), 1)
        self.assertEqual(after, before)

    def test_completed_runs_keep_missing_usage_unavailable_and_resume_without_replacement(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 1, "arm": "native"}
            calls, prompts = [], []

            def completed_call(argv, prompt, capture_dir, timeout=360):
                calls.append(argv)
                prompts.append(prompt)
                capture_dir = Path(capture_dir)
                capture_dir.mkdir(parents=True, exist_ok=True)
                (capture_dir / "events.jsonl").write_text('{"type":"turn.completed"}\n')
                (capture_dir / "stderr.log").write_text("")
                answer = Path(argv[argv.index("-o") + 1])
                answer.write_text(json.dumps({"symbols": [], "tests": [], "literals": [], "explanation": "No facts supplied."}))
                return {"exit_code": 0, "timed_out": False, "wall_seconds": 0.25}

            args = self.campaign_args(project, output, binary, julie)
            first = campaign.run_campaign(
                args, jobs=[job], auth_file=auth_file, hidden_roots=[], capture_fn=completed_call,
                redactor=lambda payload: payload,
            )
            run_dir = output / "runs/python-registration-concept-native-r1"
            outcome_before = (run_dir / "outcome.json").read_bytes()
            second = campaign.run_campaign(
                args, jobs=[job], auth_file=auth_file, hidden_roots=[],
                capture_fn=lambda *args, **kwargs: self.fail("a saved outcome must not dispatch again"),
                redactor=lambda payload: payload,
            )
            with (output / "results.csv").open(newline="") as stream:
                rows = list(csv.DictReader(stream))
            tracked = subprocess.run(["git", "-C", str(output / "seed"), "ls-files"], capture_output=True, text=True, check=True).stdout.splitlines()
            remotes = subprocess.run(["git", "-C", str(output / "seed"), "remote"], capture_output=True, text=True, check=True).stdout
            outcome_after = (run_dir / "outcome.json").read_bytes()
            saved = json.loads(outcome_after)
            events_saved = (run_dir / "attempt-001/events.jsonl").is_file()
            native_argv = " ".join(calls[0])

        self.assertEqual(first["status"], "complete")
        self.assertEqual(second["status"], "complete")
        self.assertEqual(len(calls), 1)
        self.assertEqual(outcome_after, outcome_before)
        self.assertIsNone(saved["metrics"]["input_tokens"])
        self.assertIsNone(saved["metrics"]["output_tokens"])
        self.assertIsNone(saved["billed_cost"])
        self.assertFalse(saved["mechanical_grade"]["mechanical_pass"])
        self.assertTrue(saved["mechanical_grade"]["review_required"])
        self.assertEqual(rows[0]["input_tokens"], "")
        self.assertEqual(rows[0]["acceptance"], "pending_source_review")
        self.assertEqual(saved["policy_note"], "no external-model policy declared — synthetic fixture sent to openai")
        self.assertIn("Task ID: python-registration-concept", prompts[0])
        self.assertIn("Language: python", prompts[0])
        self.assertEqual(remotes, "")
        self.assertTrue(tracked)
        self.assertFalse(any(Path(path).name in {"tasks.json", "oracles.json", "AGENTS.md", "CLAUDE.md"} for path in tracked))
        self.assertTrue(events_saved)
        self.assertNotIn(str(binary), native_argv)
        self.assertNotIn(str(julie), native_argv)
        self.assertNotIn("mcp_servers.code_kb", native_argv)

    def test_redacted_fixture_change_is_recorded_without_dispatch(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 1, "arm": "native"}
            calls = []

            def changing_redactor(payload):
                return payload.replace(b"account", b"<REDACTED>")

            def should_not_start(*args, **kwargs):
                calls.append(args)
                raise AssertionError("redaction failure must prevent dispatch")

            result = campaign.run_campaign(
                self.campaign_args(project, output, binary, julie), jobs=[job], auth_file=auth_file,
                hidden_roots=[], capture_fn=should_not_start, redactor=changing_redactor,
            )
            outcome = json.loads((output / "runs/python-registration-concept-native-r1/outcome.json").read_text())

        self.assertEqual(result["status"], "redaction_failed")
        self.assertEqual(outcome["status"], "redaction_failed")
        self.assertEqual(calls, [])

    def test_mcp_arm_preindexes_with_the_pinned_extractor_copy(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 2, "arm": "current"}
            preindex_commands, codex_commands = [], []

            def controlled_call(argv, prompt, capture_dir, timeout=360):
                if argv[0] == "env":
                    preindex_commands.append(argv)
                    return {"exit_code": 0, "timed_out": False, "wall_seconds": 1.25}
                codex_commands.append(argv)
                telemetry_setting = next(value for value in argv if value.startswith("mcp_servers.code_kb.env.CODE_KB_TELEMETRY_DIR="))
                telemetry = Path(json.loads(telemetry_setting.split("=", 1)[1]))
                telemetry.mkdir(parents=True, exist_ok=True)
                connection = sqlite3.connect(telemetry / "telemetry.db")
                connection.execute(
                    "CREATE TABLE tool_telemetry (tool TEXT, duration_ms REAL, outcome TEXT, est_tokens INTEGER, "
                    "est_tokens_saved INTEGER, est_tokens_saved_known INTEGER, result_count INTEGER, result_count_known INTEGER)"
                )
                connection.executemany(
                    "INSERT INTO tool_telemetry VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    [("file_skeleton", 12, "ok", 50, 20, 1, 4, 1), ("lookup_symbol", 24, "empty", 40, 0, 1, 0, 1)],
                )
                connection.commit()
                connection.close()
                capture_dir = Path(capture_dir)
                capture_dir.mkdir(parents=True, exist_ok=True)
                (capture_dir / "events.jsonl").write_text('{"type":"turn.completed"}\n')
                (capture_dir / "stderr.log").write_text("")
                answer = Path(argv[argv.index("-o") + 1])
                answer.write_text(json.dumps({"symbols": [], "tests": [], "literals": [], "explanation": "No facts supplied."}))
                return {"exit_code": 0, "timed_out": False, "wall_seconds": 0.5}

            result = campaign.run_campaign(
                self.campaign_args(project, output, binary, julie), jobs=[job], auth_file=auth_file,
                hidden_roots=[], capture_fn=controlled_call, redactor=lambda payload: payload,
            )
            outcome = json.loads((output / "runs/python-registration-concept-current-r1/outcome.json").read_text())
            codex_argv = " ".join(codex_commands[0])
            telemetry_rows = json.loads((output / "runs/python-registration-concept-current-r1/attempt-001/telemetry/rows.json").read_text())
            with (output / "results.csv").open(newline="") as stream:
                csv_row = next(csv.DictReader(stream))

        self.assertEqual(result["status"], "complete")
        self.assertEqual(outcome["cold_index_seconds"], 1.25)
        self.assertEqual(len(preindex_commands), 1)
        self.assertTrue(any(value.startswith("JULIE_EXTRACT_BIN=") for value in preindex_commands[0]))
        self.assertIn("mcp_servers.code_kb.command", codex_argv)
        self.assertIn("JULIE_EXTRACT_BIN", codex_argv)
        self.assertNotIn(str(binary), codex_argv)
        self.assertNotIn(str(julie), codex_argv)
        self.assertEqual([row["duration_ms"] for row in telemetry_rows], [12.0, 24.0])
        self.assertEqual(outcome["metrics"]["query_latency_samples"], 2)
        self.assertEqual(outcome["metrics"]["query_latency_p50_ms"], 12)
        self.assertEqual(outcome["metrics"]["query_latency_p95_ms"], 24)
        self.assertEqual(csv_row["query_p50_ms"], "12")
        self.assertEqual(csv_row["query_p95_ms"], "24")

    def test_provider_policy_denial_blocks_dispatch_and_is_recorded(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            (project / "AGENTS.md").write_text("## External model policy\nAllowed providers: anthropic\n")
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 1, "arm": "native"}
            calls = []

            def should_not_start(*args, **kwargs):
                calls.append(args)
                raise AssertionError("a denied provider must not be invoked")

            result = campaign.run_campaign(
                self.campaign_args(project, output, binary, julie), jobs=[job], auth_file=auth_file,
                hidden_roots=[], capture_fn=should_not_start, redactor=lambda payload: payload,
            )
            outcome = json.loads((output / "runs/python-registration-concept-native-r1/outcome.json").read_text())

        self.assertEqual(result["status"], "policy_denied")
        self.assertEqual(outcome["status"], "policy_denied")
        self.assertEqual(calls, [])

    def test_hyphenated_external_policy_heading_is_enforced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "AGENTS.md").write_text("## External-model policy\n")

            result = campaign.provider_policy(root)

        self.assertEqual(result, (False, "project policy does not allow openai"))

    def test_provider_allowlist_outside_policy_section_cannot_override_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "AGENTS.md").write_text(
                "## External model policy\nNo allowlist is declared.\n## Other\nAllowed providers: openai\n"
            )

            result = campaign.provider_policy(root)

        self.assertEqual(result, (False, "project policy does not allow openai"))

    def test_outbound_bundle_excludes_generated_fixture_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, _, _, _ = self.make_manifest_project(temporary)
            generated = project / "evals/review-2026-10-26/fixture/.code-kb/artifact.db"
            generated.parent.mkdir()
            generated.write_bytes(b"\xffsqlite index")
            task = json.loads((project / "evals/review-2026-10-26/tasks.json").read_text())["tasks"][0]

            prompt, guidance, schema, redacted = campaign.outbound_payload(
                project, task, "native", redactor=lambda payload: payload,
            )

        self.assertTrue(prompt.startswith("Task ID: python-registration-concept"))
        self.assertTrue(guidance)
        self.assertTrue(schema)
        self.assertFalse(redacted)

    def test_resume_rejects_outcome_from_a_different_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 1, "arm": "native"}
            run_dir = output / "runs/python-registration-concept-native-r1"
            run_dir.mkdir(parents=True)
            outcome_path = run_dir / "outcome.json"
            outcome_path.write_text(json.dumps({"status": "answer_received", "manifest_sha256": "old-manifest"}))

            with self.assertRaisesRegex(campaign.CampaignError, "manifest fingerprint"):
                campaign.run_campaign(
                    self.campaign_args(project, output, binary, julie), jobs=[job], auth_file=auth_file,
                    hidden_roots=[], capture_fn=lambda *args, **kwargs: self.fail("saved outcome cannot dispatch"),
                    redactor=lambda payload: payload,
                )

            self.assertEqual(json.loads(outcome_path.read_text())["manifest_sha256"], "old-manifest")

    def test_resume_preserves_old_in_progress_metadata_without_relabeling(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 1, "arm": "native"}
            run_dir = output / "runs/python-registration-concept-native-r1"
            run_dir.mkdir(parents=True)
            progress_path = run_dir / "in_progress.json"
            saved = {
                "run_id": "python-registration-concept-native-r1", "task_id": job["task_id"],
                "arm": job["arm"], "repeat": 1, "order": 1, "manifest_sha256": "old-manifest",
                "fixture_commit": "old-commit", "model": "old-model", "effort": "old-effort",
            }
            progress_path.write_text(json.dumps(saved))

            with self.assertRaisesRegex(campaign.CampaignError, "manifest fingerprint"):
                campaign.run_campaign(
                    self.campaign_args(project, output, binary, julie), jobs=[job], auth_file=auth_file,
                    hidden_roots=[], capture_fn=lambda *args, **kwargs: self.fail("old attempt cannot dispatch"),
                    redactor=lambda payload: payload,
                )

            self.assertEqual(json.loads(progress_path.read_text()), saved)
            self.assertFalse((run_dir / "outcome.json").exists())

    def test_matching_in_progress_record_becomes_interrupted_without_dispatch(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 1, "arm": "native"}
            _, manifest_hash, _, _ = campaign.validate_manifest(project, binary, julie)
            _, fixture_commit = campaign.prepare_seed(project, output)
            run_id = "python-registration-concept-native-r1"
            run_dir = output / "runs" / run_id
            run_dir.mkdir(parents=True)
            saved = {
                "run_id": run_id, "task_id": job["task_id"], "arm": job["arm"], "repeat": 1,
                "order": 1, "manifest_sha256": manifest_hash, "fixture_commit": fixture_commit,
                "model": "frozen-model", "effort": "high", "harness": "frozen-harness",
                "engine_sha256": "frozen-engine", "extractor_sha256": "frozen-extractor",
                "julie_version": "frozen-julie", "dispatch_may_occur": True,
            }
            progress_path = run_dir / "in_progress.json"
            progress_path.write_text(json.dumps(saved))

            result = campaign.run_campaign(
                self.campaign_args(project, output, binary, julie), jobs=[job], auth_file=auth_file,
                hidden_roots=[], capture_fn=lambda *args, **kwargs: self.fail("interrupted attempt cannot dispatch"),
                redactor=lambda payload: payload,
            )
            outcome = json.loads((run_dir / "outcome.json").read_text())

        self.assertEqual(result["status"], "interrupted_unknown")
        self.assertEqual({key: outcome[key] for key in saved}, saved)
        self.assertTrue(outcome["human_review_required"])

    def test_sqlite_telemetry_exports_selected_rows_and_latency_percentiles(self):
        with tempfile.TemporaryDirectory() as temporary:
            telemetry = Path(temporary)
            connection = sqlite3.connect(telemetry / "telemetry.db")
            connection.execute(
                "CREATE TABLE tool_telemetry (tool TEXT, duration_ms REAL, outcome TEXT, est_tokens INTEGER, "
                "est_tokens_saved INTEGER, est_tokens_saved_known INTEGER, result_count INTEGER, result_count_known INTEGER)"
            )
            connection.executemany(
                "INSERT INTO tool_telemetry VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                [("file_skeleton", 10, "ok", 30, 12, 1, 2, 1), ("lookup_symbol", 30, "empty", 20, 0, 1, 0, 1), ("get_symbol_body", 50, "ok", 40, 8, 1, 1, 1)],
            )
            connection.commit()
            connection.close()

            rows, error = campaign._read_telemetry(telemetry)
            summary = campaign._latency_summary(rows)
            csv_row = campaign._csv_row({
                "run_id": "r", "task_id": "t", "arm": "current", "repeat": 1, "order": 1,
                "metrics": {"query_latency_samples": summary["samples"], "query_latency_p50_ms": summary["p50_ms"], "query_latency_p95_ms": summary["p95_ms"]},
            })

        self.assertIsNone(error)
        self.assertEqual([row["tool"] for row in rows], ["file_skeleton", "lookup_symbol", "get_symbol_body"])
        self.assertEqual([row["outcome"] for row in rows], ["ok", "empty", "ok"])
        self.assertEqual(summary, {"samples": 3, "p50_ms": 30, "p95_ms": 50})
        self.assertEqual(csv_row["query_p50_ms"], 30)
        self.assertEqual(csv_row["query_p95_ms"], 50)
        self.assertIn("query_latency=available", csv_row["metric_availability"])

    def test_deadline_outcome_and_raw_error_evidence_are_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 1, "arm": "native"}

            def timed_out_call(argv, prompt, capture_dir, timeout=360):
                capture_dir = Path(capture_dir)
                capture_dir.mkdir(parents=True, exist_ok=True)
                (capture_dir / "events.jsonl").write_text('{"type":"error","message":"deadline reached"}\n')
                (capture_dir / "stderr.log").write_text("process group stopped at deadline\n")
                return {"exit_code": -15, "timed_out": True, "wall_seconds": 360.0}

            result = campaign.run_campaign(
                self.campaign_args(project, output, binary, julie),
                jobs=[job], auth_file=auth_file, hidden_roots=[], capture_fn=timed_out_call,
                redactor=lambda payload: payload,
            )
            run_dir = output / "runs/python-registration-concept-native-r1"
            outcome = json.loads((run_dir / "outcome.json").read_text())
            events = (run_dir / "attempt-001/events.jsonl").read_text()
            stderr = (run_dir / "attempt-001/stderr.log").read_text()

        self.assertEqual(result["status"], "complete")
        self.assertEqual(outcome["status"], "timed_out")
        self.assertTrue(outcome["timed_out"])
        self.assertEqual(outcome["wall_seconds"], 360.0)
        self.assertIn("deadline reached", events)
        self.assertIn("process group stopped", stderr)

    def test_retention_redaction_failure_keeps_a_nonretryable_record(self):
        with tempfile.TemporaryDirectory() as temporary:
            project, output, binary, julie = self.make_manifest_project(temporary)
            auth_file = Path(temporary) / "auth.json"
            auth_file.write_text("{}")
            job = {"task_id": "python-registration-concept", "repeat": 1, "order": 1, "arm": "native"}
            redactions = 0

            def flaky_redactor(payload):
                nonlocal redactions
                redactions += 1
                if redactions == 3:
                    raise campaign.CampaignError("outbound redaction failed")
                return payload

            def completed_call(argv, prompt, capture_dir, timeout=360):
                capture_dir = Path(capture_dir)
                capture_dir.mkdir(parents=True, exist_ok=True)
                (capture_dir / "events.jsonl").write_text('{"type":"turn.completed"}\n')
                (capture_dir / "stderr.log").write_text("")
                Path(argv[argv.index("-o") + 1]).write_text('{"symbols":[],"tests":[],"literals":[],"explanation":"No facts supplied."}')
                return {"exit_code": 0, "timed_out": False, "wall_seconds": 0.5}

            result = campaign.run_campaign(
                self.campaign_args(project, output, binary, julie), jobs=[job], auth_file=auth_file,
                hidden_roots=[], capture_fn=completed_call, redactor=flaky_redactor,
            )
            run_dir = output / "runs/python-registration-concept-native-r1"
            outcome = json.loads((run_dir / "outcome.json").read_text())
            retained_files = list((run_dir / "attempt-001").rglob("*"))

        self.assertEqual(result["status"], "retention_redaction_failed")
        self.assertEqual(outcome["task_id"], "python-registration-concept")
        self.assertEqual(outcome["arm"], "native")
        self.assertEqual(outcome["repeat"], 1)
        self.assertFalse(any(path.is_file() for path in retained_files))

    def test_default_redactor_resolves_stable_command_from_path(self):
        self.assertEqual(str(campaign.REDACTOR), "redact-outbound")
        with patch.object(campaign.shutil, "which", return_value=None):
            with self.assertRaisesRegex(campaign.CampaignError, "redact-outbound.*PATH"):
                campaign.redact_bytes(b"{}")

    def test_redact_bytes_keeps_callable_override(self):
        self.assertEqual(campaign.redact_bytes(b"payload", lambda value: value + b"!"), b"payload!")


if __name__ == "__main__":
    unittest.main()
