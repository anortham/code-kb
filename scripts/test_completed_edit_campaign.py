import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import completed_edit_campaign, comparison_campaign
from scripts import edit_evaluation, edit_snapshot
from scripts.completed_edit_campaign import CampaignError, run_campaign


ROOT = Path(__file__).resolve().parents[1]


class CompletedEditCampaignTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        compiler = shutil.which("cc")
        if compiler is None:
            raise unittest.SkipTest("a C compiler is required for the isolated fake Codex executable")
        cls.fake_root = tempfile.TemporaryDirectory(prefix="completed-edit-fake-codex-")
        cls.fake_codex = Path(cls.fake_root.name) / "codex"
        source = r'''#define _POSIX_C_SOURCE 200809L
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc >= 4 && strcmp(argv[1], "--root") == 0 && strcmp(argv[3], "scan") == 0) {
        const char *extractor = getenv("JULIE_EXTRACT_BIN");
        if (!extractor || strcmp(extractor, "/tools/intelligence/julie-extract") != 0 || access(extractor, X_OK) != 0) {
            fprintf(stderr, "EXTRACTOR_BINDING_INVALID\n");
            return 20;
        }
        fprintf(stderr, "EXTRACTOR_BOUND\n");
        if (access(".git", F_OK) == 0) fprintf(stderr, "GIT_PRESENT\n");
        if (access("tests/test_hidden.py", F_OK) != 0) fprintf(stderr, "HIDDEN_ABSENT\n");
        FILE *protected = fopen("tests/test_public.py", "w");
        if (!protected) fprintf(stderr, "PUBLIC_PROTECTED\n");
        else {
            fclose(protected);
            fprintf(stderr, "PUBLIC_WRITE_ALLOWED\n");
            return 21;
        }
        if (access("/state/fail-preindex", F_OK) == 0) {
            fprintf(stderr, "SCAN_FAILED\n");
            return 17;
        }
        if (mkdir(".code-kb", 0700) != 0) return 18;
        FILE *index = fopen(".code-kb/artifact.db", "w");
        if (!index) return 19;
        fputs("synthetic index\n", index);
        fclose(index);
        printf("SCANNED 3 files\n");
        return 0;
    }
    int timeout = 0;
    for (int i = 0; i < argc; i++) {
        if (strstr(argv[i], "FAKE_TIMEOUT")) timeout = 1;
    }
    FILE *calls = fopen("/state/fake-invocations", "a");
    if (calls) {
        fputs("invoked\n", calls);
        fclose(calls);
    }
    if (access(".git", F_OK) == 0) fprintf(stderr, "GIT_PRESENT\n");
    if (access("tests/test_hidden.py", F_OK) != 0) fprintf(stderr, "HIDDEN_ABSENT\n");
    FILE *protected = fopen("tests/test_public.py", "w");
    if (!protected) fprintf(stderr, "PUBLIC_PROTECTED\n");
    else {
        fputs("changed\n", protected);
        fclose(protected);
        fprintf(stderr, "PUBLIC_WRITE_ALLOWED\n");
    }
    if (access("/deps/fixture-cache/sentinel", R_OK) == 0) fprintf(stderr, "DEPENDENCY_MOUNTED\n");
    if (access("/tools/rust/sentinel", R_OK) == 0) fprintf(stderr, "RUST_MOUNTED\n");
    if (access("/tools/intelligence/code-kb", X_OK) == 0) fprintf(stderr, "INTELLIGENCE_MOUNTED\n");
    if (access("/tools/codex-code-mode-host", X_OK) == 0) fprintf(stderr, "CODE_MODE_HOST_MOUNTED\n");
    if (access(".code-kb/artifact.db", R_OK) == 0) fprintf(stderr, "INDEX_PRESENT\n");
    else fprintf(stderr, "INDEX_ABSENT\n");
    if (access("/state/prepared-marker", R_OK) == 0) fprintf(stderr, "RUNTIME_STATE_SHARED\n");
    const char *codex_home = getenv("CODEX_HOME");
    const char *home = getenv("HOME");
    if (codex_home || home) {
        char profile[4096];
        snprintf(profile, sizeof(profile), "%s/.codex/edit_evaluation.config.toml", codex_home ? codex_home : home);
        if (codex_home) snprintf(profile, sizeof(profile), "%s/edit_evaluation.config.toml", codex_home);
        if (access(profile, R_OK) == 0) fprintf(stderr, "PROFILE_MOUNTED\n");
    }
    FILE *project = fopen("src/app.py", "w");
    if (!project) return 21;
    fputs("def lookup(key: str):\n    return 2\n", project);
    fclose(project);
    FILE *answer = fopen("/state/final.txt", "w");
    if (answer) {
        fputs("Implemented lookup.\n", answer);
        fclose(answer);
    }
    if (access("/tools/intelligence/code-kb", X_OK) == 0) {
        printf("{\"type\":\"item.completed\",\"item\":{\"id\":\"mcp-1\",\"type\":\"mcp_tool_call\",\"server\":\"code-kb\",\"tool\":\"lookup_symbol\",\"arguments\":{\"project_root\":\"/workspace/project\",\"symbol_name\":\"lookup\",\"api_key\":\"SYNTHETIC_TRACE_SECRET\"}}}\n");
    }
    printf("{\"type\":\"item.completed\",\"item\":{\"id\":\"command-1\",\"type\":\"command_execution\",\"command\":\"python3 -m unittest tests.test_public KEY=synthetic-value\",\"exit_code\":0}}\n");
    printf("{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":123,\"output_tokens\":45,\"cached_input_tokens\":67}}}}\n");
    printf("{\"type\":\"turn.completed\",\"thread_id\":\"fake\"}\n");
    fflush(stdout);
    if (timeout) sleep(3);
    return 0;
}
'''
        subprocess.run([compiler, "-x", "c", "-", "-o", str(cls.fake_codex)], input=source, text=True, check=True)
        cls.fake_codex.chmod(0o700)

    @classmethod
    def tearDownClass(cls):
        cls.fake_root.cleanup()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="completed-edit-campaign-test-")
        self.root = Path(self.temporary.name)
        self.fixture = self.make_fixture()

    def tearDown(self):
        self.temporary.cleanup()

    def git(self, root, *arguments):
        return subprocess.run(["git", "-C", str(root), *arguments], capture_output=True, text=True, check=True).stdout.strip()

    def write(self, path, content):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def keys(self, value):
        if isinstance(value, dict):
            return set(value) | {key for item in value.values() for key in self.keys(item)}
        if isinstance(value, list):
            return {key for item in value for key in self.keys(item)}
        return set()

    def make_fixture(self, *, request="Implement lookup.", timeout_seconds=5):
        study = self.root / "study"
        case_dir = study / "cases" / "fixture"
        source_root = self.root / "source"
        seed_project = self.root / "durable-seed"
        self.write(source_root / "src/app.py", "def lookup(key: str):\n    return key\n")
        self.write(source_root / "src/helper.py", "def helper():\n    return 0\n")
        self.write(source_root / "src/__init__.py", "")
        self.write(source_root / "tests/__init__.py", "")
        self.write(source_root / "tests/test_public.py", "import unittest\nfrom src.app import lookup\n\nclass PublicTest(unittest.TestCase):\n    def test_lookup(self):\n        self.assertIsNotNone(lookup('value'))\n")
        subprocess.run(["git", "init", "--quiet", "--initial-branch=main", str(source_root)], check=True)
        self.git(source_root, "add", "--all", "--force")
        subprocess.run(
            ["git", "-C", str(source_root), "-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "fixture seed"],
            env={**os.environ, "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1"},
            check=True,
        )
        pin = self.git(source_root, "rev-parse", "HEAD")
        shutil.copytree(source_root, seed_project, symlinks=True)
        self.write(case_dir / "request.md", request)
        self.write(case_dir / "seed.patch", "")
        self.write(case_dir / "reference.patch", "")
        self.write(case_dir / "assets/test_hidden.py", "HIDDEN_ORACLE_ONLY = True\nimport unittest\nfrom src.app import lookup\n\nclass HiddenTest(unittest.TestCase):\n    def test_lookup(self):\n        self.assertEqual(lookup('value'), 2)\n")
        case = {
            "schema": 1,
            "id": "fixture",
            "repo_id": "fixture",
            "task_kind": "feature",
            "request": "request.md",
            "seed_patch": "seed.patch",
            "reference_patch": "reference.patch",
            "hidden_files": [{"path": "tests/test_hidden.py", "source": "assets/test_hidden.py"}],
            "protected_paths": [".git", "tests/test_public.py", "tests/test_hidden.py"],
            "generated_roots": [".code-kb", "target", "build"],
            "production_files": ["src/app.py", "src/helper.py"],
            "public_commands": [{"id": "public_test", "argv": ["python3", "-m", "unittest", "tests.test_public"], "cwd": ".", "timeout_seconds": 10, "expected_output": ["Ran 1 test"]}],
            "hidden_commands": [{"id": "hidden_test", "argv": ["python3", "-m", "unittest", "tests.test_hidden"], "cwd": ".", "timeout_seconds": 10, "expected_output": ["Ran 1 test"]}],
            "setup_commands": [],
            "runtime_setup_commands": [{"id": "prepare_state", "argv": ["python3", "-c", "from pathlib import Path; Path('/state/prepared-marker').write_text('prepared')"], "cwd": ".", "timeout_seconds": 10}],
            "dependency_inputs": ["fixture-cache"],
            "tool_inputs": ["rust"],
            "expected_seed_failure": {"command_id": "hidden_test", "exit_codes": [1], "diagnostic_contains": "AssertionError"},
        }
        self.write(case_dir / "case.json", json.dumps(case))
        corpus = study / "corpus.json"
        self.write(corpus, json.dumps({"repositories": [{"id": "fixture", "commit": pin, "url": "https://example.invalid/fixture"}]}))
        profiles = self.root / "profiles"
        self.write(profiles / "native/config.toml", 'model = "gpt-6.1-sol"\nmodel_reasoning_effort = "high"\n')
        self.write(profiles / "code-kb/config.toml", 'model = "gpt-6.1-sol"\nmodel_reasoning_effort = "high"\nmcp_servers.code_kb.command = "/tools/intelligence/code-kb"\n')
        dependencies = self.root / "dependency"
        self.write(dependencies / "sentinel", "offline dependency\n")
        rust = self.root / "rust"
        self.write(rust / "sentinel", "offline toolchain\n")
        intelligence = self.root / "intelligence"
        code_kb = intelligence / "code-kb"
        julie = intelligence / "julie-extract"
        companion = self.root / "codex-code-mode-host"
        redactor = self.root / "outbound-redactor"
        redactor_calls = self.root / "redactor-calls"
        intelligence.mkdir()
        shutil.copyfile(self.fake_codex, code_kb)
        self.write(julie, "fake extractor\n")
        shutil.copyfile(self.fake_codex, companion)
        code_kb.chmod(0o700)
        julie.chmod(0o700)
        companion.chmod(0o700)
        self.write(redactor, f"#!/bin/sh\nprintf x >> '{redactor_calls}'\nexec sed -E 's/key: str/key: <REDACTED>/g; s/SYNTHETIC_[A-Za-z0-9_-]+/<REDACTED>/g; s/synthetic-[A-Za-z0-9_-]+/<REDACTED>/g'\n")
        redactor.chmod(0o700)
        bindings = {
            "schema": 1,
            "intelligence_tools": str(intelligence),
            "cases": {"fixture": {"source_root": str(source_root), "dependencies": {"fixture-cache": str(dependencies)}, "tools": {"rust": str(rust)} }},
            "seeds": {"fixture": {"seed_project": str(seed_project), "seed_digest": edit_snapshot.snapshot_tree(seed_project, excluded=case["generated_roots"])["digest"], "source_pin": pin}},
        }
        bindings_path = study / "runtime-bindings.json"
        self.write(bindings_path, json.dumps(bindings))
        case_sha = edit_evaluation.hash_tree(case_dir)
        readiness_path = study / "results/readiness/fixture/readiness.json"
        readiness = {"status": "ready", "case_id": "fixture", "source_pin": pin, "case_sha256": case_sha, "seed_digest": bindings["seeds"]["fixture"]["seed_digest"], "behavioral_pass": True, "source_review": "passed"}
        self.write(readiness_path, json.dumps(readiness))
        readiness_sha = hashlib.sha256(readiness_path.read_bytes()).hexdigest()
        manifest = {
            "schema": 1,
            "status": "ready",
            "cases": [{"id": "fixture", "repo_id": "fixture", "pin": pin, "case_path": "cases/fixture", "case_sha256": case_sha, "readiness_path": "results/readiness/fixture/readiness.json", "readiness_sha256": readiness_sha}],
            "schedule": [{"case_id": "fixture", "arm": "native", "repeat": 1}, {"case_id": "fixture", "arm": "code-kb", "repeat": 1}],
            "controls": {"model": "gpt-6.1-sol", "effort": "high", "timeout_seconds": timeout_seconds},
        }
        manifest_path = study / "manifest.json"
        self.write(manifest_path, json.dumps(manifest))
        auth = self.root / "auth.json"
        self.write(auth, "{}")
        auth.chmod(0o600)
        self.fixture_artifacts = {"codex": self.fake_codex, "codex_code_mode_host": companion, "campaign_runner": Path(completed_edit_campaign.__file__).resolve(), "trace_metrics": ROOT / "scripts/comparison_metrics.py", "outbound_redactor": redactor, "profiles": profiles, "corpus": corpus, "binary": code_kb, "extractor": julie, "dependencies_fixture_fixture-cache": dependencies, "tools_fixture_rust": rust}
        self.fixture_bindings_path = bindings_path
        self.fixture_manifest_path = manifest_path
        self.fixture_output = self.root / "results"
        self.fixture_auth = auth
        self.fixture_seed = seed_project
        self.fixture_source = source_root
        self.fixture_original_source = "def lookup(key: str):\n    return key\n"
        self.fixture_redactor_calls = redactor_calls
        return case

    def execute(self, *, schedule_indices, timeout=None):
        if timeout is not None:
            manifest = json.loads(self.fixture_manifest_path.read_text())
            manifest["controls"]["timeout_seconds"] = timeout
            self.fixture_manifest_path.write_text(json.dumps(manifest))
        with patch.object(edit_evaluation, "validate_manifest", return_value={"valid": True, "case_count": 1, "scored_calls": 2}) as validate_manifest:
            result = run_campaign(
                self.fixture_manifest_path,
                self.fixture_artifacts,
                self.fixture_bindings_path,
                self.fixture_output,
                self.fixture_auth,
                schedule_indices=schedule_indices,
            )
            self.last_validation_artifacts = validate_manifest.call_args.kwargs["artifact_paths"]
            return result

    def refresh_case_binding(self):
        manifest = json.loads(self.fixture_manifest_path.read_text())
        item = manifest["cases"][0]
        case_dir = self.fixture_manifest_path.parent / item["case_path"]
        item["case_sha256"] = edit_evaluation.hash_tree(case_dir)
        readiness_path = self.fixture_manifest_path.parent / item["readiness_path"]
        readiness = json.loads(readiness_path.read_text())
        readiness["case_sha256"] = item["case_sha256"]
        readiness_path.write_text(json.dumps(readiness))
        item["readiness_sha256"] = hashlib.sha256(readiness_path.read_bytes()).hexdigest()
        self.fixture_manifest_path.write_text(json.dumps(manifest))

    def test_dispatches_both_arms_without_spending_gate_and_keeps_review_pending(self):
        result = self.execute(schedule_indices=[1, 2])

        self.assertEqual(result["status"], "finished")
        self.assertEqual([item["schedule_index"] for item in result["runs"]], [1, 2])
        self.assertEqual([item["slot_state"] for item in result["runs"]], ["dispatched", "dispatched"])
        for item in result["runs"]:
            outcome = item["outcome"]
            self.assertEqual(outcome["status"], "completed")
            self.assertTrue(outcome["trial_completed"])
            self.assertTrue(outcome["grade"]["behavioral_pass"])
            self.assertEqual(outcome["grade"]["source_review"], "pending")
            self.assertEqual(outcome["grade"]["acceptance_status"], "pending_source_review")
            self.assertFalse(outcome["grade"]["success"])
            metrics = outcome["trace_metrics"]
            self.assertEqual(metrics["input_tokens"], 123)
            self.assertEqual(metrics["output_tokens"], 45)
            self.assertEqual(metrics["cache_tokens"], 67)
            self.assertEqual(metrics["tool_calls"], 1 if outcome["arm"] == "native" else 2)
            self.assertEqual(metrics["command_count"], 1)
            self.assertEqual(metrics["mcp_call_count"], 0 if outcome["arm"] == "native" else 1)
            self.assertEqual(metrics["code_kb_calls"], 0 if outcome["arm"] == "native" else 1)
            self.assertNotIn("SYNTHETIC_TRACE_SECRET", json.dumps(metrics))
            self.assertTrue(metrics["completed"])
            self.assertEqual(len(metrics["commands"]), 1)
            self.assertIn("tests.test_public", metrics["commands"][0])
            if outcome["arm"] == "code-kb":
                self.assertEqual(metrics["mcp_calls"][0]["tool"], "lookup_symbol")
                self.assertIn("arguments", metrics["mcp_calls"][0])
                self.assertEqual(metrics["mcp_calls"][0]["arguments"]["api_key"], "<REDACTED>")
            self.assertTrue(outcome["source_audit"]["redactor_changed"])
            self.assertTrue(outcome["source_audit"]["seed_unchanged"])
            attempt = Path(item["attempt_path"])
            prompt = (attempt / "outbound_prompt.txt").read_text()
            stderr = (attempt / "evidence/agent/stderr.log").read_text()
            stdout = (attempt / "evidence/agent/stdout.log").read_text()
            trace_events = [json.loads(line) for line in stdout.splitlines()]
            agent_project = Path(outcome["agent_project"])
            self.assertNotIn("HIDDEN_ORACLE_ONLY", prompt)
            self.assertIn("public_test", prompt)
            self.assertIn("/deps/fixture-cache", prompt)
            self.assertNotIn(str(self.fixture_source), prompt)
            self.assertIn("PUBLIC_PROTECTED", stderr)
            self.assertIn("HIDDEN_ABSENT", stderr)
            self.assertIn("DEPENDENCY_MOUNTED", stderr)
            self.assertIn("RUST_MOUNTED", stderr)
            self.assertIn("CODE_MODE_HOST_MOUNTED", stderr)
            self.assertIn("RUNTIME_STATE_SHARED", stderr)
            self.assertIn("PROFILE_MOUNTED", stderr)
            self.assertNotIn("SYNTHETIC_TRACE_SECRET", stdout)
            self.assertEqual(len(trace_events), 4 if outcome["arm"] == "code-kb" else 3)
            if outcome["arm"] == "native":
                self.assertNotIn("INTELLIGENCE_MOUNTED", stderr)
                self.assertIn("INDEX_ABSENT", stderr)
                self.assertEqual(outcome["indexing"]["status"], "not_applicable")
                self.assertIsNone(outcome["indexing"]["elapsed_seconds"])
                self.assertIsNone(outcome["timing"]["index_wall_seconds"])
                self.assertEqual(outcome["timing"]["index_inclusive_wall_seconds"], outcome["timing"]["agent_wall_seconds"])
            else:
                self.assertIn("INTELLIGENCE_MOUNTED", stderr)
                self.assertIn("INDEX_PRESENT", stderr)
                self.assertEqual(outcome["indexing"]["status"], "completed")
                self.assertIsInstance(outcome["indexing"]["elapsed_seconds"], (int, float))
                self.assertEqual(outcome["indexing"]["source_digest_before"], outcome["indexing"]["source_digest_after"])
                self.assertEqual(outcome["indexing"]["source_digest_before"], outcome["source_audit"]["seed_digest"])
                self.assertTrue(Path(outcome["indexing"]["stdout_log"]).is_file())
                indexing_stderr = Path(outcome["indexing"]["stderr_log"]).read_text()
                self.assertIn("EXTRACTOR_BOUND", indexing_stderr)
                self.assertIn("PUBLIC_PROTECTED", indexing_stderr)
                self.assertIn("HIDDEN_ABSENT", indexing_stderr)
                timing = outcome["timing"]
                self.assertEqual(timing["index_wall_seconds"], outcome["indexing"]["elapsed_seconds"])
                self.assertAlmostEqual(timing["index_inclusive_wall_seconds"], timing["agent_wall_seconds"] + timing["index_wall_seconds"])
            self.assertEqual((Path(outcome["seed_project"]) / "src/app.py").read_text(), self.fixture_original_source)
            self.assertEqual((agent_project / "tests/test_public.py").read_text(), (Path(outcome["seed_project"]) / "tests/test_public.py").read_text())
            self.assertFalse((agent_project / "tests/test_hidden.py").exists())
            self.assertNotIn("authorized.json", " ".join(str(path) for path in attempt.rglob("*")))
            forbidden = ("budget", "spend", "financial", "authorization", "retry_cap", "job_cap")
            self.assertFalse(any(any(term in key.lower() for term in forbidden) for key in self.keys(outcome)))

    def test_timeout_preserves_partial_patch_and_never_promotes_behavioral_pass(self):
        request = self.fixture_manifest_path.parent / "cases/fixture/request.md"
        request.write_text("Implement lookup. FAKE_TIMEOUT")
        self.refresh_case_binding()
        result = self.execute(schedule_indices=[1], timeout=0.1)
        outcome = result["runs"][0]["outcome"]

        self.assertEqual(outcome["status"], "timed_out")
        self.assertFalse(outcome["trial_completed"])
        self.assertTrue(outcome["bundle"]["changes"])
        self.assertTrue(outcome["grade"]["behavioral_pass"])
        self.assertEqual(outcome["grade"]["acceptance_status"], "incomplete")
        self.assertEqual(outcome["grade"]["source_review"], "pending")
        self.assertFalse(outcome["grade"]["success"])

    def test_prompt_redaction_change_prevents_dispatch_without_changing_the_seed(self):
        request = self.fixture_manifest_path.parent / "cases/fixture/request.md"
        request.write_text("Implement lookup(key: str) exactly.")
        self.refresh_case_binding()

        result = self.execute(schedule_indices=[1])
        outcome = result["runs"][0]["outcome"]
        attempt = Path(result["runs"][0]["attempt_path"])

        self.assertEqual(outcome["status"], "privacy_preflight_failed")
        self.assertTrue(outcome["prompt_redaction"]["changed"])
        self.assertTrue(outcome["source_audit"]["seed_unchanged"])
        self.assertTrue((attempt / "outbound_prompt.redacted.txt").is_file())
        self.assertFalse((attempt / "outbound_prompt.txt").exists())
        self.assertFalse((attempt / "work/state-agent/fake-invocations").exists())

    def test_matching_secret_environment_value_prevents_dispatch_and_is_not_recorded(self):
        secret = "SYNTHETIC_SECRET_VALUE_2026"
        request = self.fixture_manifest_path.parent / "cases/fixture/request.md"
        request.write_text(f"Implement lookup. Private environment value: {secret}")
        self.refresh_case_binding()

        with patch.dict(os.environ, {"SYNTHETIC_API_TOKEN": secret}):
            result = self.execute(schedule_indices=[1])
        outcome = result["runs"][0]["outcome"]
        attempt = Path(result["runs"][0]["attempt_path"])

        self.assertEqual(outcome["status"], "privacy_preflight_failed")
        self.assertEqual(outcome["secret_environment_names"], ["SYNTHETIC_API_TOKEN"])
        self.assertNotIn(secret, json.dumps(outcome))
        self.assertNotIn(secret, (attempt / "outbound_prompt.redacted.txt").read_text())
        self.assertFalse((attempt / "work/state-agent/fake-invocations").exists())

    def test_profile_redaction_change_prevents_dispatch(self):
        profile = self.fixture_artifacts["profiles"] / "native/config.toml"
        profile.write_text(profile.read_text() + 'developer_instructions = "key: str"\n')

        result = self.execute(schedule_indices=[1])
        outcome = result["runs"][0]["outcome"]
        attempt = Path(result["runs"][0]["attempt_path"])

        self.assertEqual(outcome["status"], "privacy_preflight_failed")
        self.assertTrue(outcome["profile_redaction"]["changed"])
        self.assertFalse((attempt / "work/state-agent/fake-invocations").exists())

    def test_durable_seed_digest_mismatch_prevents_campaign_dispatch(self):
        bindings = json.loads(self.fixture_bindings_path.read_text())
        bindings["seeds"]["fixture"]["seed_digest"] = "0" * 64
        self.fixture_bindings_path.write_text(json.dumps(bindings))

        with patch.object(edit_evaluation, "validate_manifest", return_value={"valid": True, "case_count": 1, "scored_calls": 2}):
            with self.assertRaisesRegex(CampaignError, "durable seed digest changed"):
                run_campaign(
                    self.fixture_manifest_path,
                    self.fixture_artifacts,
                    self.fixture_bindings_path,
                    self.fixture_output,
                    self.fixture_auth,
                    schedule_indices=[1],
                )

        self.assertFalse(self.fixture_output.exists())

    def test_completed_slot_is_skipped_without_redispatch(self):
        first = self.execute(schedule_indices=[1])
        attempt = Path(first["runs"][0]["attempt_path"])
        calls_path = attempt / "work/state-agent/fake-invocations"
        self.assertEqual(calls_path.read_text().splitlines(), ["invoked"])

        second = self.execute(schedule_indices=[1])

        self.assertEqual(second["runs"][0]["slot_state"], "existing")
        self.assertEqual(calls_path.read_text().splitlines(), ["invoked"])

    def test_in_progress_slot_is_not_redispatched(self):
        self.fixture_output.mkdir()
        manifest_hash = hashlib.sha256(self.fixture_manifest_path.read_bytes()).hexdigest()
        runner_hash = hashlib.sha256(Path(completed_edit_campaign.__file__).read_bytes()).hexdigest()
        runtime_hash = hashlib.sha256(self.fixture_bindings_path.read_bytes()).hexdigest()
        (self.fixture_output / "campaign.json").write_text(json.dumps({"schema": 1, "manifest_sha256": manifest_hash, "campaign_runner_sha256": runner_hash, "runtime_bindings_sha256": runtime_hash, "schedule_count": 2}))
        attempt = self.fixture_output / "attempts/001-fixture-native-r1"
        attempt.mkdir(parents=True)
        identity = {"schema": 1, "manifest_sha256": manifest_hash, "campaign_runner_sha256": runner_hash, "runtime_bindings_sha256": runtime_hash, "schedule_index": 1, "case_id": "fixture", "arm": "native", "repeat": 1}
        (attempt / "status.json").write_text(json.dumps({**identity, "state": "dispatching"}))

        result = self.execute(schedule_indices=[1])

        self.assertEqual(result["status"], "in_progress")
        self.assertEqual(result["runs"][0]["slot_state"], "in_progress")
        self.assertFalse((attempt / "work/state-agent/fake-invocations").exists())

    def test_codex_binaries_cannot_overlap_case_sdk_mounts(self):
        overlapping_codex = self.root / "rust/bin/codex"
        overlapping_codex.parent.mkdir(parents=True)
        shutil.copyfile(self.fake_codex, overlapping_codex)
        overlapping_codex.chmod(0o700)
        self.fixture_artifacts["codex"] = overlapping_codex

        with patch.object(edit_evaluation, "validate_manifest", return_value={"valid": True, "case_count": 1, "scored_calls": 2}):
            with self.assertRaisesRegex(CampaignError, "overlap"):
                run_campaign(
                    self.fixture_manifest_path,
                    self.fixture_artifacts,
                    self.fixture_bindings_path,
                    self.fixture_output,
                    self.fixture_auth,
                    schedule_indices=[1],
                )

    def test_trace_metrics_artifact_must_match_the_active_summary_source(self):
        self.fixture_artifacts["trace_metrics"] = self.fixture_manifest_path

        with patch.object(edit_evaluation, "validate_manifest", return_value={"valid": True, "case_count": 1, "scored_calls": 2}):
            with self.assertRaisesRegex(CampaignError, "trace metrics artifact"):
                run_campaign(
                    self.fixture_manifest_path,
                    self.fixture_artifacts,
                    self.fixture_bindings_path,
                    self.fixture_output,
                    self.fixture_auth,
                    schedule_indices=[1],
                )

        self.assertFalse(self.fixture_output.exists())

    def test_preexisting_seed_index_prevents_native_campaign_dispatch(self):
        (self.fixture_seed / ".code-kb").mkdir()

        with patch.object(edit_evaluation, "validate_manifest", return_value={"valid": True, "case_count": 1, "scored_calls": 2}):
            with self.assertRaisesRegex(CampaignError, "preexisting code-kb index"):
                run_campaign(
                    self.fixture_manifest_path,
                    self.fixture_artifacts,
                    self.fixture_bindings_path,
                    self.fixture_output,
                    self.fixture_auth,
                    schedule_indices=[1],
                )

        self.assertFalse(self.fixture_output.exists())

    def test_code_kb_preindex_failure_is_retained_without_dispatch(self):
        case_path = self.fixture_manifest_path.parent / "cases/fixture/case.json"
        case = json.loads(case_path.read_text())
        case["runtime_setup_commands"][0]["argv"][2] = "from pathlib import Path; Path('/state/prepared-marker').write_text('prepared'); Path('/state/fail-preindex').write_text('fail')"
        case_path.write_text(json.dumps(case))
        self.refresh_case_binding()

        result = self.execute(schedule_indices=[2])
        run = result["runs"][0]
        outcome = run["outcome"]

        self.assertEqual(outcome["status"], "preindex_failed")
        self.assertFalse(outcome["dispatch_started"])
        self.assertEqual(outcome["indexing"]["status"], "failed")
        self.assertEqual(outcome["indexing"]["exit_status"], 17)
        self.assertIn("SCAN_FAILED", Path(outcome["indexing"]["stderr_log"]).read_text())
        self.assertFalse((Path(run["attempt_path"]) / "work/state-agent/fake-invocations").exists())

    def test_runtime_created_index_prevents_native_dispatch(self):
        case_path = self.fixture_manifest_path.parent / "cases/fixture/case.json"
        case = json.loads(case_path.read_text())
        case["runtime_setup_commands"][0]["argv"][2] = "from pathlib import Path; Path('.code-kb').mkdir(); Path('.code-kb/artifact.db').write_text('stale')"
        case_path.write_text(json.dumps(case))
        self.refresh_case_binding()

        result = self.execute(schedule_indices=[1])
        outcome = result["runs"][0]["outcome"]

        self.assertEqual(outcome["status"], "preexisting_index")
        self.assertFalse(outcome["dispatch_started"])
        self.assertFalse((Path(result["runs"][0]["attempt_path"]) / "work/state-agent/fake-invocations").exists())

    def test_interrupted_preindex_slot_is_not_repeated(self):
        self.fixture_output.mkdir()
        manifest_hash = hashlib.sha256(self.fixture_manifest_path.read_bytes()).hexdigest()
        runner_hash = hashlib.sha256(Path(completed_edit_campaign.__file__).read_bytes()).hexdigest()
        runtime_hash = hashlib.sha256(self.fixture_bindings_path.read_bytes()).hexdigest()
        (self.fixture_output / "campaign.json").write_text(json.dumps({"schema": 1, "manifest_sha256": manifest_hash, "campaign_runner_sha256": runner_hash, "runtime_bindings_sha256": runtime_hash, "schedule_count": 2}))
        attempt = self.fixture_output / "attempts/002-fixture-code-kb-r1"
        attempt.mkdir(parents=True)
        identity = {"schema": 1, "manifest_sha256": manifest_hash, "campaign_runner_sha256": runner_hash, "runtime_bindings_sha256": runtime_hash, "schedule_index": 2, "case_id": "fixture", "arm": "code-kb", "repeat": 1}
        (attempt / "status.json").write_text(json.dumps({**identity, "state": "preindexing", "dispatch_may_occur": False}))

        result = self.execute(schedule_indices=[2])

        self.assertEqual(result["status"], "in_progress")
        self.assertEqual(result["runs"][0]["slot_state"], "in_progress")
        self.assertFalse((attempt / "work/state-agent/fake-invocations").exists())
        self.assertFalse((attempt / "evidence/indexing/stdout.log").exists())

    def test_retained_jsonl_stays_parseable_and_keeps_sanitized_actions(self):
        raw_path = self.root / "raw.stdout"
        retained_path = self.root / "evidence/agent/stdout.log"
        raw_path.parent.mkdir(parents=True, exist_ok=True)
        raw = (
            b'{"type":"item.completed","item":{"id":"mcp-1","type":"mcp_tool_call","server":"code-kb","tool":"lookup_symbol","arguments":{"api_key":"SYNTHETIC_TRACE_SECRET"}}}\n'
            b'{"type":"item.completed","item":{"id":"command-1","type":"command_execution","command":"cargo test KEY=synthetic-value","exit_code":0}}\n'
            b'malformed KEY=synthetic-malformed\n'
        )
        raw_path.write_bytes(raw)

        redactor = self.fixture_artifacts["outbound_redactor"]
        trace = completed_edit_campaign._safe_trace(raw_path, retained_path, jsonl=True, redactor=redactor)
        metrics = completed_edit_campaign._trace_metrics(raw, redactor=redactor)

        events = [json.loads(line) for line in retained_path.read_text().splitlines()]
        self.assertEqual(len(events), 2)
        self.assertEqual(events[0]["item"]["tool"], "lookup_symbol")
        self.assertNotIn("SYNTHETIC_TRACE_SECRET", retained_path.read_text())
        self.assertEqual(trace["status"], "partial")
        self.assertEqual(trace["malformed_lines"], 1)
        malformed = Path(trace["malformed_path"])
        self.assertTrue(malformed.is_file())
        self.assertNotIn("synthetic-malformed", malformed.read_text())
        self.assertEqual(metrics["commands"], ["cargo test KEY=<REDACTED>"])
        self.assertEqual(metrics["mcp_calls"][0]["tool"], "lookup_symbol")
        self.assertEqual(metrics["mcp_calls"][0]["arguments"]["api_key"], "<REDACTED>")
        self.assertNotIn("SYNTHETIC_TRACE_SECRET", json.dumps(metrics))

    def test_completed_launcher_uses_the_validated_redactor_artifact(self):
        result = self.execute(schedule_indices=[1])
        outcome = result["runs"][0]["outcome"]

        self.assertGreater(self.fixture_redactor_calls.stat().st_size, 0)
        self.assertEqual(self.last_validation_artifacts["outbound_redactor"], self.fixture_artifacts["outbound_redactor"])
        self.assertIs(completed_edit_campaign.comparison_campaign, comparison_campaign)
        self.assertEqual(str(comparison_campaign.REDACTOR), "redact-outbound")
        self.assertEqual(outcome["source_audit"]["redactor_status"], "complete")

    def test_completed_launcher_requires_an_executable_redactor_artifact(self):
        redactor = self.fixture_artifacts["outbound_redactor"]
        redactor.chmod(0o600)

        with self.assertRaisesRegex(CampaignError, "outbound redactor must be an executable"):
            self.execute(schedule_indices=[1])

    def test_failed_log_retention_keeps_raw_files_without_missing_safe_paths(self):
        raw_command = self.root / "command.stdout"
        raw_grade = self.root / "grade.stdout"
        raw_command.write_text("private command output")
        raw_grade.write_text("private grade output")
        records = [{"stdout_log": str(raw_command)}]
        grade = {"checks": {"public": {"commands": [{"stdout_log": str(raw_grade)}]}}}

        def failed_redactor(payload):
            raise CampaignError("outbound redaction failed")

        completed_edit_campaign._sanitize_command_records(records, self.root / "safe-command", "setup", redactor=failed_redactor)
        completed_edit_campaign._sanitize_grade_logs(grade, self.root / "safe-grade", redactor=failed_redactor)

        command_record = records[0]
        grade_record = grade["checks"]["public"]["commands"][0]
        self.assertEqual(command_record["stdout_log"], str(raw_command))
        self.assertEqual(grade_record["stdout_log"], str(raw_grade))
        self.assertTrue(raw_command.is_file())
        self.assertTrue(raw_grade.is_file())
        self.assertEqual(command_record["retained_traces"]["stdout_log"]["status"], "unavailable")
        self.assertTrue(command_record["retained_traces"]["stdout_log"]["raw_retained"])
        self.assertEqual(grade_record["retained_traces"]["stdout_log"]["status"], "unavailable")
        self.assertTrue(grade_record["retained_traces"]["stdout_log"]["raw_retained"])
        self.assertFalse((self.root / "safe-command/setup-00-stdout_log.log").exists())
        self.assertFalse((self.root / "safe-grade/grade-000-stdout_log.log").exists())

    def test_trace_read_failure_reports_unavailable_and_keeps_raw_file(self):
        raw_path = self.root / "unreadable.stdout"
        raw_path.write_text("private trace")

        with patch.object(Path, "read_bytes", side_effect=OSError("injected trace read failure")):
            trace = completed_edit_campaign._safe_trace(raw_path, self.root / "safe.stdout", redactor=lambda payload: payload)

        self.assertEqual(trace["status"], "unavailable")
        self.assertEqual(trace["error"], "injected trace read failure")
        self.assertTrue(trace["raw_retained"])
        self.assertTrue(raw_path.is_file())
        self.assertIsNone(trace["path"])

    def test_successful_log_retention_replaces_and_removes_raw_files(self):
        raw_command = self.root / "successful-command.stdout"
        raw_grade = self.root / "successful-grade.stdout"
        raw_command.write_text("command output")
        raw_grade.write_text("grade output")
        records = [{"stdout_log": str(raw_command)}]
        grade = {"checks": {"public": {"commands": [{"stdout_log": str(raw_grade)}]}}}

        completed_edit_campaign._sanitize_command_records(records, self.root / "safe-command", "setup", redactor=lambda payload: payload)
        completed_edit_campaign._sanitize_grade_logs(grade, self.root / "safe-grade", redactor=lambda payload: payload)
        completed_edit_campaign._sanitize_command_records(records, self.root / "safe-command", "setup", redactor=lambda payload: payload)
        completed_edit_campaign._sanitize_grade_logs(grade, self.root / "safe-grade", redactor=lambda payload: payload)

        command_record = records[0]
        grade_record = grade["checks"]["public"]["commands"][0]
        self.assertFalse(raw_command.exists())
        self.assertFalse(raw_grade.exists())
        self.assertTrue(Path(command_record["stdout_log"]).is_file())
        self.assertTrue(Path(grade_record["stdout_log"]).is_file())
        self.assertEqual(command_record["retained_traces"]["stdout_log"]["status"], "complete")
        self.assertEqual(grade_record["retained_traces"]["stdout_log"]["status"], "complete")

    def test_grader_interruption_keeps_timing_metadata_and_does_not_redispatch(self):
        with patch.object(edit_evaluation, "replay_and_grade", side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                self.execute(schedule_indices=[2])

        attempt = self.fixture_output / "attempts/002-fixture-code-kb-r1"
        status_path = attempt / "status.json"
        saved = json.loads(status_path.read_text())
        invocation_log = attempt / "work/state-agent/fake-invocations"
        invocations_before_resume = invocation_log.read_text()

        self.assertEqual(saved["state"], "grading")
        self.assertNotEqual(saved["source_audit"]["source_sha256"], saved["source_audit"]["redacted_sha256"])
        self.assertEqual(saved["source_audit"]["redactor_status"], "complete")
        self.assertEqual(len(saved["runtime_setup"]), 1)
        self.assertTrue(saved["runtime_setup"][0]["passed"])
        self.assertEqual(saved["indexing"]["status"], "completed")
        self.assertIsInstance(saved["timing"]["agent_wall_seconds"], (int, float))
        self.assertIsInstance(saved["timing"]["index_wall_seconds"], (int, float))
        self.assertTrue(saved["execution"]["turn_completed_event"])
        self.assertEqual(saved["execution"]["exit_status"], 0)
        self.assertNotIn("SYNTHETIC_TRACE_SECRET", status_path.read_text())
        self.assertNotIn("Implemented lookup", status_path.read_text())

        resumed = self.execute(schedule_indices=[2])

        self.assertEqual(resumed["runs"][0]["slot_state"], "in_progress")
        self.assertEqual(invocation_log.read_text(), invocations_before_resume)

    def test_trace_write_failure_preserves_private_logs_and_resume_does_not_redispatch(self):
        with patch.object(completed_edit_campaign, "_atomic_trace_write", side_effect=OSError("injected trace write failure")):
            with patch.object(edit_evaluation, "replay_and_grade", side_effect=KeyboardInterrupt):
                with self.assertRaises(KeyboardInterrupt):
                    self.execute(schedule_indices=[1])

        attempt = self.fixture_output / "attempts/001-fixture-native-r1"
        private_root = attempt / "private-raw"
        raw_logs = private_root / "agent-output"
        stdout = raw_logs / "stdout.log"
        stderr = raw_logs / "stderr.log"
        status_path = attempt / "status.json"
        saved = json.loads(status_path.read_text())
        invocation_log = attempt / "work/state-agent/fake-invocations"
        invocations_before_resume = invocation_log.read_text()

        self.assertEqual(saved["state"], "grading")
        self.assertEqual(saved["execution"]["stdout_redaction"]["status"], "unavailable")
        self.assertEqual(saved["execution"]["stderr_redaction"]["status"], "unavailable")
        self.assertTrue(saved["execution"]["stdout_redaction"]["raw_retained"])
        self.assertEqual(saved["execution"]["stdout_redaction"]["error"], "injected trace write failure")
        self.assertTrue(stdout.is_file())
        self.assertTrue(stderr.is_file())
        self.assertEqual(private_root.stat().st_mode & 0o777, 0o700)
        self.assertEqual(stdout.stat().st_mode & 0o777, 0o600)
        self.assertEqual(stderr.stat().st_mode & 0o777, 0o600)
        self.assertNotIn("SYNTHETIC_TRACE_SECRET", status_path.read_text())

        resumed = self.execute(schedule_indices=[1])

        self.assertEqual(resumed["runs"][0]["slot_state"], "in_progress")
        self.assertEqual(invocation_log.read_text(), invocations_before_resume)


if __name__ == "__main__":
    unittest.main()
