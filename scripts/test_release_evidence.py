import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("release_evidence.py")


class ReleaseEvidenceTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name) / "repo"
        self.repo.mkdir()
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Release Evidence Test")
        self.git("config", "user.email", "release-evidence@example.test")
        self.write("Cargo.toml", '[workspace.package]\nversion = "1.0.0"\n')
        self.write(".gitignore", "target/\n")
        self.write(".cargo/config.toml", "[build]\njobs = 1\n")
        self.write("rust-toolchain.toml", '[toolchain]\nchannel = "stable"\n')
        self.write("README.md", "Initial documentation.\n")
        self.write("crates/code-kb-core/src/lib.rs", "pub fn query() -> u32 { 1 }\n")
        self.write(
            "crates/code-kb-core/Cargo.toml",
            '[package]\nname = "code-kb-core"\nversion = "0.1.0"\n',
        )
        self.write(
            "crates/code-kb-cli/Cargo.toml",
            '[package]\nname = "code-kb-cli"\nversion.workspace = true\n\n'
            '[dependencies]\ncode-kb-core = { version = "0.1.0", path = "../code-kb-core" }\n',
        )
        self.write("crates/code-kb-cli/src/main.rs", "fn main() {}\n")
        self.write("scripts/reach_gate.py", "def score():\n    return 1\n")
        self.write("scripts/release_evidence.py", "")
        self.write("scripts/test_reach_gate.py", "")
        self.write("scripts/test_release_evidence.py", "")
        self.write("scripts/julie-pins.json", json.dumps({"version": "3.9.1"}))
        self.write(
            "scripts/release-evidence-inputs.json",
            json.dumps(
                {
                    "schema": 1,
                    "files": [
                        "Cargo.lock",
                        "Cargo.toml",
                        "crates/code-kb-core/Cargo.toml",
                        "crates/code-kb-cli/Cargo.toml",
                        ".cargo/config.toml",
                        "rust-toolchain.toml",
                        "crates/code-kb-core/src/lib.rs",
                        "scripts/reach_gate.py",
                        "scripts/release_evidence.py",
                        "scripts/release-dogfood.sh",
                        "scripts/test_reach_gate.py",
                        "scripts/test_release_evidence.py",
                        "scripts/julie-pins.json",
                        "scripts/release-evidence-inputs.json",
                        "evals/reach/corpus.json",
                        "evals/languages/fixtures.json",
                    ],
                    "globs": [
                        "crates/code-kb-core/**/*.rs",
                        "crates/code-kb-cli/**/*.rs",
                        "evals/reach/cases/**/*.json",
                        "evals/review-2026-10-26/**/*",
                        "evals/languages/**/*",
                    ],
                    "exclude_globs": [
                        "**/.git/**",
                        "**/.code-kb/**",
                        "**/target/**",
                        "**/tmp/**",
                        "**/build/**",
                        "**/dist/**",
                        "**/node_modules/**",
                        "**/__pycache__/**",
                    ],
                    "normalize_cargo_package_versions": True,
                    "normalize_cargo_path_dependency_versions": [
                        {
                            "file": "crates/code-kb-cli/Cargo.toml",
                            "section": "dependencies",
                            "name": "code-kb-core",
                            "path": "../code-kb-core",
                        }
                    ],
                    "extractor_level": "references",
                    "test_commands": [
                        "python3 -m unittest scripts.test_reach_gate scripts.test_release_evidence"
                    ],
                }
            ),
        )
        self.write(
            "Cargo.lock",
            'version = 4\n\n[[package]]\nname = "code-kb-core"\nversion = "0.1.0"\n\n'
            '[[package]]\nname = "fixture-dep"\nversion = "1.2.3"\n'
            'source = "registry+https://example.test"\n',
        )
        self.write("scripts/release-dogfood.sh", "#!/usr/bin/env bash\n")
        self.write("evals/reach/corpus.json", "[]\n")
        self.write("evals/reach/cases/demo.json", "{}\n")
        self.write("evals/languages/fixtures.json", '{"python":"sample.py"}\n')
        self.write("evals/languages/python/sample.py", "print('fixture')\n")
        self.write("evals/review-2026-10-26/tasks.json", "[]\n")
        self.test_command = f'{sys.executable} -c "print(\'regression test passed\')"'
        inputs = json.loads((self.repo / "scripts/release-evidence-inputs.json").read_text())
        inputs["test_commands"] = [self.test_command]
        self.write("scripts/release-evidence-inputs.json", json.dumps(inputs))
        extractor = self.repo / ".tools" / "julie-extract"
        extractor.parent.mkdir(parents=True)
        extractor.write_text("#!/bin/sh\nprintf 'julie-extract 3.9.1\\n'\n")
        extractor.chmod(0o755)
        self.extractor = extractor
        code_kb = self.repo / "bin" / "code-kb"
        code_kb.parent.mkdir(parents=True)
        code_kb.write_text("#!/bin/sh\nprintf 'lookup result\\n'\n")
        code_kb.chmod(0o755)
        self.code_kb = code_kb
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "baseline")
        self.base_commit = self.git("rev-parse", "HEAD")

    def write(self, relative, content):
        path = self.repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        return path

    def git(self, *args):
        return subprocess.run(
            ["git", *args], cwd=self.repo, capture_output=True, text=True, check=True
        ).stdout.strip()

    def command(self, *args):
        self.assertTrue(SCRIPT.exists(), "the shared release evidence implementation is missing")
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            cwd=self.repo,
            capture_output=True,
            text=True,
            check=False,
        )

    def capture(self):
        output = self.repo / "target" / "evidence.json"
        done = self.command(
            "capture",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--out",
            str(output),
        )
        self.assertEqual(done.returncode, 0, done.stderr)
        return json.loads(output.read_text())

    def reach_report(self, evidence):
        report = self.repo / "target" / "reach" / "report.json"
        report.parent.mkdir(parents=True, exist_ok=True)
        commit = self.git("rev-parse", "HEAD")
        report.write_text(
            json.dumps(
                {
                    "result": "PASS",
                    "commit": commit,
                    "source_commit": commit,
                    "tested_binary": {
                        "path": str(self.code_kb.resolve()),
                        "sha256": hashlib.sha256(self.code_kb.read_bytes()).hexdigest(),
                    },
                    "evidence": evidence,
                }
            )
        )
        return report

    def dogfood_report(self, round_commit, body=""):
        report = self.repo / "target" / "dogfood" / "report.md"
        report.parent.mkdir(parents=True, exist_ok=True)
        evidence_path = report.parent / "evidence.json"
        evidence = json.loads(evidence_path.read_text())
        if "sessions" not in evidence:
            session_dir = report.parent / evidence["session_dir"]
            session_dir.mkdir(parents=True, exist_ok=True)
            (session_dir / "claude.md").write_text("Claude session fixture output.\n")
            (session_dir / "codex.md").write_text("Codex session fixture output.\n")
            finalized = self.command("finalize-dogfood", "--evidence", str(evidence_path))
            self.assertEqual(finalized.returncode, 0, finalized.stderr)
            evidence = json.loads(evidence_path.read_text())
        tested_binary = evidence["tested_binary"]
        sessions = evidence["sessions"]
        report.write_text(
            "# Release dogfood report\n\n"
            f"Commit: {round_commit}\n"
            f"Source commit: {evidence['source_commit']}\n"
            "New: code-kb 1.0.0, julie-extract 3.9.1\n\n"
            f"Sessions: {sessions['claude']['path']}, {sessions['codex']['path']}\n"
            f"Claude transcript: {sessions['claude']['path']}\n"
            f"Claude transcript SHA-256: {sessions['claude']['sha256']}\n"
            f"Codex transcript: {sessions['codex']['path']}\n"
            f"Codex transcript SHA-256: {sessions['codex']['sha256']}\n"
            f"Input fingerprint: {evidence['snapshot']['fingerprint']}\n"
            f"Tested binary: {tested_binary['path']}\n"
            f"Tested binary SHA-256: {tested_binary['sha256']}\n"
            "Commit identifies source HEAD; it does not prove how the tested binary was built.\n\n"
            "## Defects\n\n"
            f"{body}\n"
            "## Result: PASS\n"
        )
        return report

    def finalize_dogfood(self, evidence):
        value = json.loads(evidence.read_text())
        session_dir = evidence.parent / value["session_dir"]
        session_dir.mkdir(parents=True, exist_ok=True)
        (session_dir / "claude.md").write_text("Claude session fixture output.\n")
        (session_dir / "codex.md").write_text("Codex session fixture output.\n")
        return self.command("finalize-dogfood", "--evidence", str(evidence))

    def start_dogfood(self):
        evidence = self.repo / "target" / "dogfood" / "evidence.json"
        evidence.parent.mkdir(parents=True, exist_ok=True)
        session_dir = evidence.parent / "20261026T000000Z"
        session_dir.mkdir(parents=True, exist_ok=True)
        done = self.command(
            "start-dogfood",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--binary",
            str(self.code_kb),
            "--session-dir",
            str(session_dir),
            "--out",
            str(evidence),
        )
        self.assertEqual(done.returncode, 0, done.stderr)
        self.session_dir = session_dir
        return evidence

    def validate_dogfood(self, evidence, report):
        return self.command(
            "validate-dogfood",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--evidence",
            str(evidence),
            "--report",
            str(report),
        )

    def test_reach_evidence_is_tied_to_source_and_grade_inputs(self):
        original = self.capture()
        report = self.reach_report(original)
        valid = self.command(
            "validate-reach",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--report",
            str(report),
        )
        self.assertEqual(valid.returncode, 0, valid.stderr)

        self.write("crates/code-kb-core/src/lib.rs", "pub fn query() -> u32 { 2 }\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "change behavior")
        stale = self.command(
            "validate-reach",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--report",
            str(report),
        )
        self.assertEqual(stale.returncode, 1)
        self.assertIn("crates/code-kb-core/src/lib.rs", stale.stderr)

    def test_build_configuration_and_language_fixtures_invalidate_reach_evidence(self):
        baseline = self.capture()
        changes = {
            "Cargo.toml": '[workspace.package]\nversion = "1.0.0"\n\n[workspace.dependencies]\nfixture-dep = "2.0.0"\n',
            "crates/code-kb-core/Cargo.toml": '[package]\nname = "code-kb-core"\nversion = "0.1.0"\n\n[dependencies]\nfixture-dep = "2.0.0"\n',
            "Cargo.lock": (self.repo / "Cargo.lock").read_text().replace('version = "1.2.3"', 'version = "1.2.4"'),
            ".cargo/config.toml": "[build]\njobs = 2\n",
            "rust-toolchain.toml": '[toolchain]\nchannel = "1.86.0"\n',
            "evals/languages/fixtures.json": '{"python":"sample.py","rust":"sample.rs"}\n',
            "evals/languages/python/sample.py": "print('changed fixture')\n",
        }
        for relative, content in changes.items():
            with self.subTest(path=relative):
                self.write(relative, content)
                current = self.capture()
                self.assertNotEqual(baseline["fingerprint"], current["fingerprint"])
                self.assertNotEqual(baseline["inputs"][relative], current["inputs"][relative])
                original = {
                    "Cargo.toml": '[workspace.package]\nversion = "1.0.0"\n',
                    "crates/code-kb-core/Cargo.toml": '[package]\nname = "code-kb-core"\nversion = "0.1.0"\n',
                    "Cargo.lock": 'version = 4\n\n[[package]]\nname = "code-kb-core"\nversion = "0.1.0"\n\n[[package]]\nname = "fixture-dep"\nversion = "1.2.3"\nsource = "registry+https://example.test"\n',
                    ".cargo/config.toml": "[build]\njobs = 1\n",
                    "rust-toolchain.toml": '[toolchain]\nchannel = "stable"\n',
                    "evals/languages/fixtures.json": '{"python":"sample.py"}\n',
                    "evals/languages/python/sample.py": "print('fixture')\n",
                }[relative]
                self.write(relative, original)

    def test_production_optional_build_config_inputs_are_fingerprinted_and_reject_untracked_config(self):
        production_manifest = json.loads(
            (SCRIPT.parents[1] / "scripts/release-evidence-inputs.json").read_text()
        )
        optional_globs = set(production_manifest.get("optional_globs", []))
        self.assertIn("scripts/julie-pins.json", production_manifest["files"])
        self.assertIn("crates/code-kb-cli/**/*.rs", production_manifest["globs"])
        expected = {
            ".cargo/config",
            ".cargo/config.toml",
            "rust-toolchain",
            "rust-toolchain.toml",
        }
        self.assertTrue(expected <= optional_globs)

        fixture_manifest_path = self.repo / "scripts/release-evidence-inputs.json"
        fixture_manifest = json.loads(fixture_manifest_path.read_text())
        fixture_manifest["files"] = [
            path for path in fixture_manifest["files"] if path not in expected
        ]
        fixture_manifest["optional_globs"] = sorted(optional_globs)
        self.write(fixture_manifest_path.relative_to(self.repo), json.dumps(fixture_manifest))
        self.write("crates/code-kb-cli/build.rs", "fn main() {}\n")
        self.git("add", "scripts/release-evidence-inputs.json")
        self.git("add", "crates/code-kb-cli/build.rs")
        self.git("commit", "-q", "-m", "use production optional build inputs")

        baseline = self.capture()
        self.assertIn(".cargo/config.toml", baseline["inputs"])
        self.assertIn("rust-toolchain.toml", baseline["inputs"])
        self.assertIn("crates/code-kb-cli/build.rs", baseline["inputs"])
        self.assertIn("scripts/julie-pins.json", baseline["inputs"])
        self.write(".cargo/config.toml", "[build]\njobs = 2\n")
        changed = self.capture()
        self.assertNotEqual(baseline["fingerprint"], changed["fingerprint"])

        self.write(".cargo/config.toml", "[build]\njobs = 1\n")
        self.write("crates/code-kb-cli/build.rs", "fn main() { let _ = 1; }\n")
        changed_build_script = self.capture()
        self.assertNotEqual(baseline["fingerprint"], changed_build_script["fingerprint"])

        self.write("crates/code-kb-cli/build.rs", "fn main() {}\n")
        self.git("rm", "-q", ".cargo/config.toml", "rust-toolchain.toml")
        self.git("commit", "-q", "-m", "remove optional build config")
        no_config = self.capture()
        self.assertNotIn(".cargo/config.toml", no_config["inputs"])
        self.assertNotIn("rust-toolchain.toml", no_config["inputs"])

        self.write(".cargo/config", "[build]\njobs = 3\n")
        rejected = self.command(
            "start-dogfood",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--binary",
            str(self.code_kb),
            "--session-dir",
            str(self.repo / "target/dogfood/round"),
            "--out",
            str(self.repo / "target/dogfood/evidence.json"),
        )
        self.assertEqual(rejected.returncode, 1)
        self.assertIn(".cargo/config", rejected.stderr)

    def test_package_version_fields_are_normalized_without_masking_dependencies(self):
        baseline = self.capture()
        self.write("Cargo.toml", '[workspace.package]\nversion = "1.0.1"\n')
        root_bump = self.capture()
        self.assertEqual(baseline["fingerprint"], root_bump["fingerprint"])

        self.write(
            "crates/code-kb-core/Cargo.toml",
            '[package]\nname = "code-kb-core"\nversion = "0.1.1"\n',
        )
        lock = (self.repo / "Cargo.lock").read_text().replace(
            'name = "code-kb-core"\nversion = "0.1.0"',
            'name = "code-kb-core"\nversion = "0.1.1"',
        )
        self.write("Cargo.lock", lock)
        cli_manifest = (self.repo / "crates/code-kb-cli/Cargo.toml").read_text().replace(
            'version = "0.1.0"', 'version = "0.1.1"'
        )
        self.write("crates/code-kb-cli/Cargo.toml", cli_manifest)
        package_bump = self.capture()
        self.assertEqual(baseline["fingerprint"], package_bump["fingerprint"])

        self.write(
            "crates/code-kb-core/Cargo.toml",
            '[package]\nname = "code-kb-core"\nversion = "0.1.1"\n\n[dependencies]\nfixture-dep = "2.0.0"\n',
        )
        dependency_change = self.capture()
        self.assertNotEqual(baseline["fingerprint"], dependency_change["fingerprint"])

    def test_git_index_selects_semantic_inputs_and_ignores_generated_rust(self):
        baseline = self.capture()
        generated = self.repo / "crates/code-kb-core/target/tmp/generated.rs"
        generated.parent.mkdir(parents=True)
        generated.write_text("pub fn generated() {}\n")
        after_generated = self.capture()
        self.assertEqual(baseline["fingerprint"], after_generated["fingerprint"])
        self.assertNotIn("crates/code-kb-core/target/tmp/generated.rs", after_generated["inputs"])

        untracked = "crates/code-kb-core/src/untracked.rs"
        self.write(untracked, "pub fn untracked() {}\n")
        before_stage = self.capture()
        self.assertNotIn(untracked, before_stage["inputs"])
        rejected = self.command(
            "start-dogfood",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--binary",
            str(self.code_kb),
            "--session-dir",
            str(self.repo / "target/dogfood/round"),
            "--out",
            str(self.repo / "target/dogfood/evidence.json"),
        )
        self.assertEqual(rejected.returncode, 1)
        self.assertIn(untracked, rejected.stderr)

        self.git("add", untracked)
        staged = self.capture()
        self.assertIn(untracked, staged["inputs"])
        self.assertNotEqual(baseline["fingerprint"], staged["fingerprint"])
        staged_rejected = self.command(
            "start-dogfood",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--binary",
            str(self.code_kb),
            "--session-dir",
            str(self.repo / "target/dogfood/round"),
            "--out",
            str(self.repo / "target/dogfood/evidence.json"),
        )
        self.assertEqual(staged_rejected.returncode, 1)
        self.assertIn(untracked, staged_rejected.stderr)

    def test_docs_and_package_version_changes_do_not_invalidate_reach_evidence(self):
        report = self.reach_report(self.capture())
        self.write("README.md", "Updated release instructions.\n")
        self.write("Cargo.toml", '[workspace.package]\nversion = "1.0.1"\n')
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "docs and version")
        done = self.command(
            "validate-reach",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--report",
            str(report),
        )
        self.assertEqual(done.returncode, 0, done.stderr)

    def test_extractor_identity_and_test_command_changes_invalidate_evidence(self):
        report = self.reach_report(self.capture())
        extractor_bytes = self.extractor.read_text().replace("3.9.1", "3.9.1 ")
        self.extractor.write_text(extractor_bytes)
        stale_extractor = self.command(
            "validate-reach",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--report",
            str(report),
        )
        self.assertEqual(stale_extractor.returncode, 1)
        self.assertIn("extractor", stale_extractor.stderr.lower())

        self.extractor.write_text(extractor_bytes.replace("3.9.1 ", "3.9.1"))
        inputs = json.loads((self.repo / "scripts/release-evidence-inputs.json").read_text())
        inputs["test_commands"] = ["python3 -m unittest scripts.test_reach_gate"]
        self.write("scripts/release-evidence-inputs.json", json.dumps(inputs))
        stale_config = self.command(
            "validate-reach",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--report",
            str(report),
        )
        self.assertEqual(stale_config.returncode, 1)
        self.assertIn("release-evidence-inputs.json", stale_config.stderr)

    def test_malformed_missing_and_future_reach_evidence_fail_with_reasons(self):
        report = self.reach_report(self.capture())
        report.write_text("{")
        malformed = self.command(
            "validate-reach",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--report",
            str(report),
        )
        self.assertEqual(malformed.returncode, 1)
        self.assertIn("malformed", malformed.stderr.lower())

        missing = self.command(
            "validate-reach",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--report",
            str(self.repo / "target/missing.json"),
        )
        self.assertEqual(missing.returncode, 1)
        self.assertIn("missing", missing.stderr.lower())

        future = self.reach_report({"schema": 999, "fingerprint": "sha256:future"})
        future_result = self.command(
            "validate-reach",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--report",
            str(future),
        )
        self.assertEqual(future_result.returncode, 1)
        self.assertIn("schema", future_result.stderr.lower())

    def test_dogfood_needs_local_reproduction_for_each_later_input_commit(self):
        evidence = self.start_dogfood()
        report = self.dogfood_report(self.base_commit)
        self.write("crates/code-kb-core/src/lib.rs", "pub fn query() -> u32 { 2 }\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "unreviewed semantic change")
        fix_commit = self.git("rev-parse", "HEAD")

        missing_fix = self.validate_dogfood(evidence, report)
        self.assertEqual(missing_fix.returncode, 1)
        self.assertIn("reproduction evidence", missing_fix.stderr.lower())

        report.write_text(
            report.read_text().replace(
                "## Result: PASS",
                f"- [fixed {fix_commit}] query output regression: fixed\n\n## Result: PASS",
            )
        )
        recorded = self.command(
            "record-fix",
            "--repo",
            str(self.repo),
            "--evidence",
            str(evidence),
            "--report",
            str(report),
            "--commit",
            fix_commit,
            "--defect",
            "query output regression",
            "--extractor",
            str(self.extractor),
            "--test-command",
            self.test_command,
            "--exact-call-json",
            json.dumps([str(self.code_kb), "--root", str(self.repo), "lookup", "query"]),
        )
        self.assertEqual(recorded.returncode, 0, recorded.stderr)
        self.assertEqual(report.read_text().count(f"- [fixed {fix_commit}]"), 1)
        fix = json.loads(evidence.read_text())["fixes"][0]
        self.assertEqual(fix["source_commit"], fix_commit)
        self.assertRegex(fix["snapshot"]["fingerprint"], r"^sha256:[0-9a-f]{64}$")
        self.assertEqual(
            fix["snapshot"]["inputs"]["crates/code-kb-core/src/lib.rs"],
            hashlib.sha256(b"pub fn query() -> u32 { 2 }\n").hexdigest(),
        )
        self.assertEqual(fix["tested_binary"]["path"], str(self.code_kb.resolve()))
        self.assertEqual(
            fix["tested_binary"]["sha256"],
            hashlib.sha256(self.code_kb.read_bytes()).hexdigest(),
        )
        verified = self.validate_dogfood(evidence, report)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        value = json.loads(evidence.read_text())
        value["fixes"][0]["tested_binary"]["sha256"] = "0" * 64
        evidence.write_text(json.dumps(value))
        tampered = self.validate_dogfood(evidence, report)
        self.assertEqual(tampered.returncode, 1)
        self.assertIn("no [fixed", tampered.stderr.lower())

    def test_record_fix_rejects_a_named_commit_when_head_is_later(self):
        evidence = self.start_dogfood()
        report = self.dogfood_report(self.base_commit)
        self.write("crates/code-kb-core/src/lib.rs", "pub fn query() -> u32 { 2 }\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "fix first query")
        fix_commit = self.git("rev-parse", "HEAD")
        self.write("crates/code-kb-cli/src/main.rs", "fn main() { let _ = 2; }\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "later semantic change")

        result = self.command(
            "record-fix",
            "--repo",
            str(self.repo),
            "--evidence",
            str(evidence),
            "--report",
            str(report),
            "--commit",
            fix_commit,
            "--defect",
            "first query regression",
            "--extractor",
            str(self.extractor),
            "--test-command",
            self.test_command,
            "--exact-call-json",
            json.dumps([str(self.code_kb), "--root", str(self.repo), "lookup", "query"]),
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("HEAD", result.stderr)
        self.assertEqual(json.loads(evidence.read_text())["fixes"], [])

    def test_record_fix_rejects_non_query_code_kb_commands(self):
        evidence = self.start_dogfood()
        report = self.dogfood_report(self.base_commit)
        self.write("crates/code-kb-core/src/lib.rs", "pub fn query() -> u32 { 2 }\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "semantic change")
        commit = self.git("rev-parse", "HEAD")
        rejected_calls = [
            ["--version"],
            ["--help"],
            ["serve"],
            ["hook"],
            ["scan"],
            ["stats"],
            ["telemetry"],
        ]
        for suffix in rejected_calls:
            with self.subTest(command=suffix):
                result = self.command(
                    "record-fix",
                    "--repo",
                    str(self.repo),
                    "--evidence",
                    str(evidence),
                    "--report",
                    str(report),
                    "--commit",
                    commit,
                    "--defect",
                    "query regression",
                    "--extractor",
                    str(self.extractor),
                    "--test-command",
                    self.test_command,
                    "--exact-call-json",
                    json.dumps([str(self.code_kb), "--root", str(self.repo), *suffix]),
                )
                self.assertEqual(result.returncode, 1)
                self.assertIn("query/report", result.stderr.lower())

    def test_record_fix_requires_one_absolute_root_and_an_in_root_database(self):
        evidence = self.start_dogfood()
        report = self.dogfood_report(self.base_commit)
        self.write("crates/code-kb-core/src/lib.rs", "pub fn query() -> u32 { 2 }\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "semantic change")
        commit = self.git("rev-parse", "HEAD")
        outside_db = Path(self.tmp.name) / "outside.db"
        outside_db.write_bytes(b"not an index")
        cases = [
            (["lookup", "--root", str(self.repo), "query"], "absolute --root"),
            (["lookup", "--root", "relative", "query"], "absolute --root"),
            (["lookup", "--db", str(outside_db), "query"], "inside --root"),
        ]
        for suffix, expected in cases:
            with self.subTest(call=suffix):
                result = self.command(
                    "record-fix",
                    "--repo",
                    str(self.repo),
                    "--evidence",
                    str(evidence),
                    "--report",
                    str(report),
                    "--commit",
                    commit,
                    "--defect",
                    "query regression",
                    "--extractor",
                    str(self.extractor),
                    "--test-command",
                    self.test_command,
                    "--exact-call-json",
                    json.dumps([str(self.code_kb), "--root", str(self.repo), *suffix]),
                )
                self.assertEqual(result.returncode, 1)
                self.assertIn(expected, result.stderr)

    def test_one_fix_commit_can_record_each_exact_call_it_repairs(self):
        evidence = self.start_dogfood()
        report = self.dogfood_report(self.base_commit)
        self.write("crates/code-kb-core/src/lib.rs", "pub fn query() -> u32 { 2 }\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "shared semantic fix")
        commit = self.git("rev-parse", "HEAD")
        for defect in ("first wrong lookup", "second wrong lookup"):
            recorded = self.command(
                "record-fix",
                "--repo",
                str(self.repo),
                "--evidence",
                str(evidence),
                "--report",
                str(report),
                "--commit",
                commit,
                "--defect",
                defect,
                "--extractor",
                str(self.extractor),
                "--test-command",
                self.test_command,
                "--exact-call-json",
                json.dumps([str(self.code_kb), "--root", str(self.repo), "lookup", defect]),
            )
            self.assertEqual(recorded.returncode, 0, recorded.stderr)
        verified = self.validate_dogfood(evidence, report)
        self.assertEqual(verified.returncode, 0, verified.stderr)

    def test_fix_record_requires_a_reproduced_code_kb_cli_call(self):
        evidence = self.start_dogfood()
        report = self.dogfood_report(self.base_commit)
        self.write("crates/code-kb-core/src/lib.rs", "pub fn query() -> u32 { 2 }\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "semantic change")
        commit = self.git("rev-parse", "HEAD")
        done = self.command(
            "record-fix",
            "--repo",
            str(self.repo),
            "--evidence",
            str(evidence),
            "--report",
            str(report),
            "--commit",
            commit,
            "--defect",
            "query output regression",
            "--extractor",
            str(self.extractor),
            "--test-command",
            self.test_command,
            "--exact-call-json",
            json.dumps([sys.executable, "-c", "print('not a code-kb call')"]),
        )
        self.assertEqual(done.returncode, 1)
        self.assertIn("code-kb CLI", done.stderr)

    def test_dogfood_allows_docs_and_version_commits_without_a_second_round(self):
        evidence = self.start_dogfood()
        report = self.dogfood_report(self.base_commit)
        self.write("README.md", "Updated instructions.\n")
        self.write("Cargo.toml", '[workspace.package]\nversion = "1.0.1"\n')
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "docs and version")
        valid = self.validate_dogfood(evidence, report)
        self.assertEqual(valid.returncode, 0, valid.stderr)

    def test_missing_malformed_and_future_dogfood_evidence_fail_with_reasons(self):
        evidence = self.start_dogfood()
        report = self.dogfood_report(self.base_commit)
        valid = self.validate_dogfood(evidence, report)
        self.assertEqual(valid.returncode, 0, valid.stderr)

        missing = self.validate_dogfood(evidence.with_name("missing.json"), report)
        self.assertEqual(missing.returncode, 1)
        self.assertIn("missing", missing.stderr.lower())

        evidence.write_text("{")
        malformed = self.validate_dogfood(evidence, report)
        self.assertEqual(malformed.returncode, 1)
        self.assertIn("malformed", malformed.stderr.lower())

        evidence.write_text(json.dumps({"schema": 999}))
        future = self.validate_dogfood(evidence, report)
        self.assertEqual(future.returncode, 1)
        self.assertIn("future", future.stderr.lower())

    def test_dogfood_pass_requires_one_result_and_commit_line(self):
        evidence = self.start_dogfood()
        report = self.dogfood_report(self.base_commit)
        original = report.read_text()
        invalid_reports = (
            original.replace("## Result: PASS", "## Result: PASS\n## Result: PENDING", 1),
            original.replace(
                f"Commit: {self.base_commit}",
                f"Commit: {self.base_commit}\nCommit: {self.base_commit}",
                1,
            ),
        )
        for text in invalid_reports:
            with self.subTest(report=text):
                report.write_text(text)
                rejected = self.validate_dogfood(evidence, report)
                self.assertEqual(rejected.returncode, 1)
                self.assertIn("exactly one", rejected.stderr.lower())

    def test_dogfood_finalization_requires_both_nonempty_session_outputs(self):
        evidence = self.start_dogfood()
        sidecar = json.loads(evidence.read_text())
        premature_report = evidence.parent / "premature-report.md"
        premature_report.write_text(
            f"Commit: {self.base_commit}\n"
            f"Source commit: {self.base_commit}\n"
            f"Input fingerprint: {sidecar['snapshot']['fingerprint']}\n"
            f"Tested binary: {sidecar['tested_binary']['path']}\n"
            f"Tested binary SHA-256: {sidecar['tested_binary']['sha256']}\n"
            "## Result: PASS\n"
        )
        premature = self.validate_dogfood(evidence, premature_report)
        self.assertEqual(premature.returncode, 1)
        self.assertIn("transcripts must be finalized", premature.stderr)

        missing = self.command("finalize-dogfood", "--evidence", str(evidence))
        self.assertEqual(missing.returncode, 1)
        self.assertIn("claude.md", missing.stderr)

        claude = self.session_dir / "claude.md"
        codex = self.session_dir / "codex.md"
        claude.write_text("  \n")
        codex.write_text("Codex session output.\n")
        empty = self.command("finalize-dogfood", "--evidence", str(evidence))
        self.assertEqual(empty.returncode, 1)
        self.assertIn("empty", empty.stderr.lower())

        claude.write_text("Claude session output.\n")
        finalized = self.command("finalize-dogfood", "--evidence", str(evidence))
        self.assertEqual(finalized.returncode, 0, finalized.stderr)
        value = json.loads(evidence.read_text())
        self.assertEqual(
            value["sessions"]["claude"]["path"],
            "20261026T000000Z/claude.md",
        )
        self.assertEqual(
            value["sessions"]["claude"]["sha256"],
            hashlib.sha256(claude.read_bytes()).hexdigest(),
        )

    def test_dogfood_validation_rejects_changed_and_outside_transcripts(self):
        evidence = self.start_dogfood()
        finalized = self.finalize_dogfood(evidence)
        self.assertEqual(finalized.returncode, 0, finalized.stderr)
        report = self.dogfood_report(self.base_commit)
        claude = self.session_dir / "claude.md"
        original = claude.read_bytes()
        claude.write_text("Edited after finalization.\n")
        changed = self.validate_dogfood(evidence, report)
        self.assertEqual(changed.returncode, 1)
        self.assertIn("changed claude session transcript", changed.stderr.lower())

        claude.write_bytes(original)
        report_text = report.read_text()
        claude_hash = json.loads(evidence.read_text())["sessions"]["claude"]["sha256"]
        report.write_text(
            report_text.replace(
                f"Claude transcript SHA-256: {claude_hash}",
                "Claude transcript SHA-256: " + "0" * 64,
                1,
            )
        )
        report_hash_mismatch = self.validate_dogfood(evidence, report)
        self.assertEqual(report_hash_mismatch.returncode, 1)
        self.assertIn("Claude transcript SHA-256", report_hash_mismatch.stderr)
        report.write_text(report_text)

        value = json.loads(evidence.read_text())
        value["sessions"]["codex"]["path"] = "../outside/codex.md"
        evidence.write_text(json.dumps(value))
        outside = self.validate_dogfood(evidence, report)
        self.assertEqual(outside.returncode, 1)
        self.assertIn("outside", outside.stderr.lower())

    def test_dogfood_validation_rejects_changed_and_outside_transcripts(self):
        evidence = self.start_dogfood()
        finalized = self.finalize_dogfood(evidence)
        self.assertEqual(finalized.returncode, 0, finalized.stderr)
        report = self.dogfood_report(self.base_commit)
        claude = self.session_dir / "claude.md"
        original = claude.read_bytes()
        claude.write_text("Edited after finalization.\n")
        changed = self.validate_dogfood(evidence, report)
        self.assertEqual(changed.returncode, 1)
        self.assertIn("changed claude session transcript", changed.stderr.lower())

        claude.write_bytes(original)
        value = json.loads(evidence.read_text())
        value["sessions"]["codex"]["path"] = "../outside/codex.md"
        evidence.write_text(json.dumps(value))
        outside = self.validate_dogfood(evidence, report)
        self.assertEqual(outside.returncode, 1)
        self.assertIn("outside", outside.stderr.lower())

    def test_start_dogfood_records_source_head_and_tested_binary_hash_separately(self):
        session_dir = self.repo / "target/dogfood/20261026T000000Z"
        session_dir.mkdir(parents=True, exist_ok=True)
        evidence = self.command(
            "start-dogfood",
            "--repo",
            str(self.repo),
            "--extractor",
            str(self.extractor),
            "--binary",
            str(self.code_kb),
            "--session-dir",
            str(session_dir),
            "--out",
            str(self.repo / "target/dogfood/evidence.json"),
        )
        self.assertEqual(evidence.returncode, 0, evidence.stderr)
        value = json.loads((self.repo / "target/dogfood/evidence.json").read_text())
        self.assertEqual(value["source_commit"], self.base_commit)
        self.assertEqual(value["round_commit"], self.base_commit)
        self.assertEqual(value["tested_binary"]["path"], str(self.code_kb.resolve()))
        self.assertEqual(
            value["tested_binary"]["sha256"],
            hashlib.sha256(self.code_kb.read_bytes()).hexdigest(),
        )

    def test_report_renderer_keeps_record_fix_example_literal(self):
        output = self.repo / "target/dogfood/report.md"
        output.parent.mkdir(parents=True)
        evidence = self.start_dogfood()
        finalized = self.finalize_dogfood(evidence)
        self.assertEqual(finalized.returncode, 0, finalized.stderr)
        fake_bin = Path(self.tmp.name) / "fake-bin"
        fake_bin.mkdir()
        marker = Path(self.tmp.name) / "python-ran"
        python = fake_bin / "python3"
        python.write_text("#!/bin/sh\n: > \"$DOGFOOD_SUBSTITUTION_MARKER\"\nexit 97\n")
        python.chmod(0o755)
        env = os.environ.copy()
        env["PATH"] = f"{fake_bin}:{env.get('PATH', '')}"
        env["DOGFOOD_SUBSTITUTION_MARKER"] = str(marker)
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "render-dogfood-report",
                "--evidence",
                str(evidence),
                "--out",
                str(output),
                "--code-kb-version",
                "0.1.0",
                "--julie-version",
                "3.9.1",
                "--round",
                "20261026T000000Z",
            ],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(marker.exists())
        text = output.read_text()
        self.assertIn("Commit identifies source HEAD", text)
        self.assertIn("Claude transcript SHA-256: ", text)
        self.assertIn("Codex transcript SHA-256: ", text)
        self.assertIn(
            "Tested binary SHA-256: "
            + hashlib.sha256(self.code_kb.read_bytes()).hexdigest(),
            text,
        )
        self.assertIn("`python3 scripts/release_evidence.py record-fix", text)
        release_script = Path(__file__).with_name("release-dogfood.sh").read_text()
        self.assertIn("render-dogfood-report", release_script)
        self.assertIn("finalize-dogfood", release_script)
        self.assertLess(release_script.index("finalize-dogfood"), release_script.index("render-dogfood-report"))
        self.assertNotRegex(release_script, r"<<\s*EOF\b")
        output.write_text(
            text.replace("## Result: PENDING", "## Result: PASS").replace(
                "- [open] <defect>", "- [gap] no open fixture defects"
            )
        )
        valid = self.validate_dogfood(evidence, output)
        self.assertEqual(valid.returncode, 0, valid.stderr)


if __name__ == "__main__":
    unittest.main()
