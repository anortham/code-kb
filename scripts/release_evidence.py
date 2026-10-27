#!/usr/bin/env python3
"""Capture and validate the source inputs behind release evaluation reports."""

import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
from pathlib import Path, PurePosixPath


SCHEMA = 1
CONFIG_PATH = "scripts/release-evidence-inputs.json"
MAX_OUTPUT_CHARS = 3000
COMMAND_TIMEOUT_SECONDS = 900
EXACT_CALL_COMMANDS = {
    "outline",
    "skeleton",
    "lookup",
    "symbol",
    "search",
    "body",
    "context",
    "slice",
    "refs",
    "blast-radius",
    "impact",
    "facts",
    "check",
}


class EvidenceError(ValueError):
    pass


def read_json(path, label):
    try:
        return json.loads(Path(path).read_text())
    except FileNotFoundError as error:
        raise EvidenceError(f"missing {label}: {path}") from error
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise EvidenceError(f"malformed {label} at {path}: {error}") from error


def git(repo, *args, check=True):
    result = subprocess.run(
        ["git", *args], cwd=repo, capture_output=True, text=True, check=False
    )
    if check and result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        raise EvidenceError(f"git {' '.join(args)} failed: {detail}")
    return result


def config_for(repo):
    path = Path(repo) / CONFIG_PATH
    config = read_json(path, "release evidence input configuration")
    if not isinstance(config, dict) or config.get("schema") != SCHEMA:
        raise EvidenceError(f"unsupported or malformed release evidence input configuration: {path}")
    files = config.get("files")
    globs = config.get("globs")
    optional_globs = config.get("optional_globs", [])
    exclude_globs = config.get("exclude_globs", [])
    commands = config.get("test_commands")
    level = config.get("extractor_level")
    if not isinstance(files, list) or not all(isinstance(item, str) for item in files):
        raise EvidenceError(f"release evidence configuration has no valid files list: {path}")
    if not isinstance(globs, list) or not all(isinstance(item, str) for item in globs):
        raise EvidenceError(f"release evidence configuration has no valid globs list: {path}")
    if not isinstance(optional_globs, list) or not all(isinstance(item, str) for item in optional_globs):
        raise EvidenceError(f"release evidence configuration has no valid optional_globs list: {path}")
    if not isinstance(exclude_globs, list) or not all(isinstance(item, str) for item in exclude_globs):
        raise EvidenceError(f"release evidence configuration has no valid exclude_globs list: {path}")
    normalize_versions = config.get("normalize_cargo_package_versions", False)
    if not isinstance(normalize_versions, bool):
        raise EvidenceError(f"invalid normalize_cargo_package_versions in {path}")
    local_path_versions = config.get("normalize_cargo_path_dependency_versions", [])
    required_local_path_fields = {"file", "section", "name", "path"}
    if not isinstance(local_path_versions, list) or any(
        not isinstance(item, dict)
        or not required_local_path_fields <= item.keys()
        or not all(isinstance(item[key], str) and item[key] for key in required_local_path_fields)
        for item in local_path_versions
    ):
        raise EvidenceError(f"invalid normalize_cargo_path_dependency_versions in {path}")
    if not isinstance(commands, list) or not commands or not all(isinstance(item, str) and item for item in commands):
        raise EvidenceError(f"release evidence configuration has no valid test_commands list: {path}")
    if level not in {"references", "facts", "full"}:
        raise EvidenceError(f"unsupported extractor level in {path}: {level!r}")
    pin_file = config.get("extractor_pin_file", "scripts/julie-pins.json")
    if not isinstance(pin_file, str) or not pin_file:
        raise EvidenceError(f"invalid extractor_pin_file in {path}")
    return config


def normalized_relative(path):
    relative = PurePosixPath(path).as_posix()
    if relative.startswith("/") or ".." in PurePosixPath(relative).parts:
        raise EvidenceError(f"release evidence input must stay inside the repository: {path}")
    return relative


def glob_matches(path, pattern):
    expression = ["^"]
    index = 0
    while index < len(pattern):
        pair = pattern[index : index + 2]
        if pair == "**":
            if index + 2 < len(pattern) and pattern[index + 2] == "/":
                expression.append("(?:.*/)?")
                index += 3
                continue
            expression.append(".*")
            index += 2
            continue
        character = pattern[index]
        expression.append("[^/]*" if character == "*" else "[^/]" if character == "?" else re.escape(character))
        index += 1
    expression.append("$")
    return re.match("".join(expression), path) is not None


def input_path_matches(path, config):
    path = PurePosixPath(path).as_posix()
    if any(glob_matches(path, pattern) for pattern in config.get("exclude_globs", [])):
        return False
    if path == CONFIG_PATH or path in config["files"]:
        return True
    patterns = [*config["globs"], *config.get("optional_globs", [])]
    return any(glob_matches(path, pattern) for pattern in patterns)


def cargo_package_names(root, manifest_paths):
    names = set()
    for relative in manifest_paths:
        if Path(relative).name != "Cargo.toml":
            continue
        section = None
        for line in (root / relative).read_text().splitlines():
            section_match = re.match(r"^\s*\[([^\]]+)\]\s*(?:#.*)?$", line)
            if section_match:
                section = section_match.group(1)
            elif section == "package":
                name_match = re.match(r'^\s*name\s*=\s*"([^"]+)"\s*(?:#.*)?$', line)
                if name_match:
                    names.add(name_match.group(1))
    return names


def normalize_cargo_manifest(contents, relative, config):
    normalized = []
    section = None
    for line in contents.decode("utf-8").splitlines(keepends=True):
        section_match = re.match(r"^\s*\[([^\]]+)\]\s*(?:#.*)?(?:\r?\n)?$", line)
        if section_match:
            section = section_match.group(1)
        if section in {"package", "workspace.package"}:
            version_match = re.match(
                r"^(\s*version\s*=\s*)([\"'])([^\"']*)([\"'])(\s*(?:#.*)?)(\r?\n)?$",
                line,
            )
            if version_match and version_match.group(2) == version_match.group(4):
                line = (
                    version_match.group(1)
                    + version_match.group(2)
                    + "<package-version>"
                    + version_match.group(4)
                    + version_match.group(5)
                    + (version_match.group(6) or "")
                )
        for item in config.get("normalize_cargo_path_dependency_versions", []):
            if relative != item["file"] or section != item["section"]:
                continue
            dependency = re.match(r"^\s*([^\s=]+)\s*=\s*\{", line)
            path_match = re.search(r"\bpath\s*=\s*([\"'])([^\"']+)\1", line)
            version_match = re.search(r"\bversion\s*=\s*([\"'])([^\"']+)([\"'])", line)
            if (
                dependency
                and dependency.group(1) == item["name"]
                and path_match
                and path_match.group(2) == item["path"]
                and version_match
                and version_match.group(1) == version_match.group(3)
            ):
                start, end = version_match.span(2)
                line = line[:start] + "<package-version>" + line[end:]
        normalized.append(line)
    return "".join(normalized).encode("utf-8")


def normalize_cargo_lock(contents, package_names):
    lines = contents.decode("utf-8").splitlines(keepends=True)
    starts = [index for index, line in enumerate(lines) if line.strip() == "[[package]]"]
    for offset, start in enumerate(starts):
        end = starts[offset + 1] if offset + 1 < len(starts) else len(lines)
        name = None
        source = None
        version_index = None
        for index in range(start + 1, end):
            line = lines[index]
            if line.lstrip().startswith("["):
                break
            name_match = re.match(r'^\s*name\s*=\s*"([^"]+)"\s*(?:#.*)?(?:\r?\n)?$', line)
            if name_match:
                name = name_match.group(1)
                continue
            if re.match(r"^\s*source\s*=", line):
                source = line
                continue
            version_match = re.match(
                r"^(\s*version\s*=\s*)([\"'])([^\"']*)([\"'])(\s*(?:#.*)?)(\r?\n)?$",
                line,
            )
            if version_match and version_index is None:
                version_index = index
        if name in package_names and source is None and version_index is not None:
            version_match = re.match(
                r"^(\s*version\s*=\s*)([\"'])([^\"']*)([\"'])(\s*(?:#.*)?)(\r?\n)?$",
                lines[version_index],
            )
            if version_match and version_match.group(2) == version_match.group(4):
                lines[version_index] = (
                    version_match.group(1)
                    + version_match.group(2)
                    + "<package-version>"
                    + version_match.group(4)
                    + version_match.group(5)
                    + (version_match.group(6) or "")
                )
    return "".join(lines).encode("utf-8")


def normalized_input_bytes(root, relative, package_names, config):
    contents = (root / relative).read_bytes()
    if not config.get("normalize_cargo_package_versions", False):
        return contents
    if Path(relative).name == "Cargo.toml":
        return normalize_cargo_manifest(contents, relative, config)
    if Path(relative).name == "Cargo.lock":
        return normalize_cargo_lock(contents, package_names)
    return contents


def input_paths(repo, config):
    root = Path(repo).resolve()
    tracked = {
        PurePosixPath(path).as_posix()
        for path in git(root, "ls-files", "--cached", "-z").stdout.split("\0")
        if path
    }
    selected = {CONFIG_PATH}
    for raw in config["files"]:
        relative = normalized_relative(raw)
        if not (root / relative).is_file():
            raise EvidenceError(f"missing release evidence input: {relative}")
        selected.add(relative)
    if not (root / CONFIG_PATH).is_file():
        raise EvidenceError(f"missing release evidence input: {CONFIG_PATH}")
    required_globs = set(config["globs"])
    patterns = [*config["globs"], *config.get("optional_globs", [])]
    for pattern in patterns:
        normalized_relative(pattern)
        excluded = config.get("exclude_globs", [])
        matches = [
            path
            for path in tracked
            if not any(glob_matches(path, item) for item in excluded)
            and glob_matches(path, pattern)
        ]
        if pattern in required_globs and not matches:
            raise EvidenceError(f"release evidence input glob matched no tracked files: {pattern}")
        selected.update(matches)
    package_names = (
        cargo_package_names(root, selected)
        if config.get("normalize_cargo_package_versions", False)
        else set()
    )
    result = {}
    for relative in sorted(selected):
        path = root / relative
        if not path.is_file():
            raise EvidenceError(f"missing release evidence input: {relative}")
        if not path.resolve().is_relative_to(root):
            raise EvidenceError(f"release evidence input escapes the repository: {relative}")
        result[relative] = hashlib.sha256(normalized_input_bytes(root, relative, package_names, config)).hexdigest()
    return result


def default_extractor(repo):
    configured = os.environ.get("JULIE_EXTRACT_BIN")
    if configured:
        return Path(configured).expanduser()
    root = Path(repo)
    for candidate in (root / ".tools/julie-extract", root / ".tools/julie-extract.exe"):
        if candidate.is_file():
            return candidate
    found = shutil.which("julie-extract")
    if found:
        return Path(found)
    raise EvidenceError("missing julie-extract; restore the pinned extractor before capturing release evidence")


def extractor_info(repo, config, extractor=None):
    root = Path(repo).resolve()
    path = Path(extractor).expanduser() if extractor else default_extractor(root)
    if not path.is_file():
        raise EvidenceError(f"missing julie-extract binary: {path}")
    resolved = path.resolve()
    version_result = subprocess.run(
        [str(resolved), "--version"], capture_output=True, text=True, check=False, timeout=15
    )
    if version_result.returncode:
        detail = version_result.stderr.strip() or version_result.stdout.strip()
        raise EvidenceError(f"cannot read julie-extract version from {path}: {detail}")
    version_text = (version_result.stdout or version_result.stderr).strip()
    match = re.search(r"\b(\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?)\b", version_text)
    if not match:
        raise EvidenceError(f"cannot parse julie-extract version from {path}: {version_text!r}")
    pin_path = root / config.get("extractor_pin_file", "scripts/julie-pins.json")
    pin = read_json(pin_path, "pinned extractor identity")
    pinned_version = pin.get("version") if isinstance(pin, dict) else None
    if not isinstance(pinned_version, str) or not pinned_version:
        raise EvidenceError(f"missing pinned julie-extract version in {pin_path}")
    if match.group(1) != pinned_version:
        raise EvidenceError(
            f"julie-extract reports {match.group(1)}, but {pin_path.relative_to(root)} pins {pinned_version}"
        )
    return {
        "version": match.group(1),
        "pinned_version": pinned_version,
        "binary_sha256": hashlib.sha256(resolved.read_bytes()).hexdigest(),
        "level": config["extractor_level"],
        "path": resolved.name,
    }


def fingerprint_payload(inputs, extractor, test_commands):
    return {"inputs": inputs, "extractor": extractor, "test_commands": test_commands}


def fingerprint_for(inputs, extractor, test_commands):
    canonical = json.dumps(
        fingerprint_payload(inputs, extractor, test_commands),
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return "sha256:" + hashlib.sha256(canonical).hexdigest()


def capture(repo, extractor=None):
    root = Path(repo).resolve()
    config = config_for(root)
    inputs = input_paths(root, config)
    julie = extractor_info(root, config, extractor)
    commands = list(config["test_commands"])
    return {
        "schema": SCHEMA,
        "fingerprint": fingerprint_for(inputs, julie, commands),
        "inputs": inputs,
        "extractor": julie,
        "test_commands": commands,
    }


def binary_identity(binary):
    path = Path(binary).expanduser().resolve()
    if not path.is_file():
        raise EvidenceError(f"missing tested code-kb binary: {path}")
    return {
        "path": str(path),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }


def validate_binary_identity(identity, label):
    if not isinstance(identity, dict):
        raise EvidenceError(f"malformed {label}: expected an object")
    path = identity.get("path")
    digest = identity.get("sha256")
    if not isinstance(path, str) or not Path(path).is_absolute():
        raise EvidenceError(f"malformed {label}: path must be absolute")
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise EvidenceError(f"malformed {label}: missing SHA-256")
    return identity


def validate_snapshot(snapshot, label):
    if not isinstance(snapshot, dict):
        raise EvidenceError(f"malformed {label}: expected an object")
    version = snapshot.get("schema")
    if not isinstance(version, int) or version != SCHEMA:
        reason = "future" if isinstance(version, int) and version > SCHEMA else "unsupported or missing"
        raise EvidenceError(f"{reason} {label} schema {version!r}; supported schema is {SCHEMA}")
    inputs = snapshot.get("inputs")
    extractor = snapshot.get("extractor")
    commands = snapshot.get("test_commands")
    digest = snapshot.get("fingerprint")
    if not isinstance(inputs, dict) or not inputs or not all(
        isinstance(path, str) and isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value)
        for path, value in inputs.items()
    ):
        raise EvidenceError(f"malformed {label}: invalid input hashes")
    if not isinstance(extractor, dict) or not all(
        isinstance(extractor.get(field), str) and extractor[field]
        for field in ("version", "pinned_version", "binary_sha256", "level")
    ):
        raise EvidenceError(f"malformed {label}: invalid extractor identity")
    if not re.fullmatch(r"[0-9a-f]{64}", extractor["binary_sha256"]):
        raise EvidenceError(f"malformed {label}: invalid extractor binary hash")
    if extractor["version"] != extractor["pinned_version"]:
        raise EvidenceError(f"malformed {label}: extractor version does not match its recorded pin")
    if extractor["level"] not in {"references", "facts", "full"}:
        raise EvidenceError(f"malformed {label}: unsupported extractor level {extractor['level']!r}")
    if not isinstance(commands, list) or not commands or not all(isinstance(item, str) and item for item in commands):
        raise EvidenceError(f"malformed {label}: invalid test commands")
    if not isinstance(digest, str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise EvidenceError(f"malformed {label}: invalid fingerprint")
    expected = fingerprint_for(inputs, extractor, commands)
    if digest != expected:
        raise EvidenceError(f"malformed {label}: fingerprint does not match its recorded inputs")
    return snapshot


def snapshot_changes(previous, current):
    before = previous["inputs"]
    after = current["inputs"]
    changes = sorted(
        path
        for path in set(before) | set(after)
        if before.get(path) != after.get(path)
    )
    if previous["extractor"] != current["extractor"]:
        changes.append("extractor identity or level")
    if previous["test_commands"] != current["test_commands"] and not any(
        path.endswith("release-evidence-inputs.json") for path in changes
    ):
        changes.append("test command configuration")
    return changes


def ensure_ancestor(repo, ancestor, descendant="HEAD"):
    if not isinstance(ancestor, str) or not re.fullmatch(r"[0-9a-f]{40}", ancestor):
        raise EvidenceError(f"malformed commit id in release evidence: {ancestor!r}")
    result = git(repo, "merge-base", "--is-ancestor", ancestor, descendant, check=False)
    if result.returncode:
        raise EvidenceError(f"evidence commit {ancestor} is not an ancestor of {descendant}")


def commit_id(repo, reference="HEAD"):
    if reference != "HEAD" and not re.fullmatch(r"[0-9a-f]{7,40}", reference):
        raise EvidenceError(f"invalid commit id: {reference!r}")
    result = git(repo, "rev-parse", f"{reference}^{{commit}}", check=False)
    if result.returncode:
        raise EvidenceError(f"cannot resolve commit {reference!r}")
    return result.stdout.strip()


def commit_paths(repo, commit):
    result = git(repo, "diff-tree", "--no-commit-id", "--name-only", "--no-renames", "-r", "-m", commit)
    return sorted({PurePosixPath(path).as_posix() for path in result.stdout.splitlines() if path})


def latest_input_commits(repo, base_commit, config):
    history = git(repo, "rev-list", "--reverse", f"{base_commit}..HEAD").stdout.splitlines()
    latest = {}
    paths_by_commit = {}
    for commit in history:
        paths = [path for path in commit_paths(repo, commit) if input_path_matches(path, config)]
        paths_by_commit[commit] = paths
        for path in paths:
            latest[path] = commit
    return latest, paths_by_commit


def ensure_inputs_clean(repo, config):
    status = git(repo, "status", "--porcelain=v1", "-z", "--untracked-files=all").stdout.split("\0")
    dirty = []
    index = 0
    while index < len(status):
        row = status[index]
        if not row:
            index += 1
            continue
        paths = [row[3:]] if len(row) >= 4 else []
        if "R" in row[:2] or "C" in row[:2]:
            index += 1
            if index < len(status) and status[index]:
                paths.append(status[index])
        for raw_path in paths:
            path = PurePosixPath(raw_path).as_posix()
            if input_path_matches(path, config):
                dirty.append(path)
        index += 1
    if dirty:
        raise EvidenceError(
            "uncommitted release evidence inputs: " + ", ".join(sorted(set(dirty)))
        )


def ensure_pass_report(report, label):
    try:
        text = Path(report).read_text()
    except FileNotFoundError as error:
        raise EvidenceError(f"missing {label} report: {report}") from error
    except (OSError, UnicodeError) as error:
        raise EvidenceError(f"malformed {label} report at {report}: {error}") from error
    result_lines = re.findall(r"(?m)^## Result:.*$", text)
    if result_lines != ["## Result: PASS"]:
        raise EvidenceError(f"{label} report must contain exactly one ## Result: PASS line")
    if re.search(r"(?m)^- \[open\]", text):
        raise EvidenceError(f"{label} report lists an open defect")
    commit_lines = re.findall(r"(?m)^Commit:.*$", text)
    if len(commit_lines) != 1:
        raise EvidenceError(f"{label} report must contain exactly one Commit line")
    match = re.fullmatch(r"Commit: ([0-9a-f]{40})", commit_lines[0])
    if not match:
        raise EvidenceError(f"{label} report has no valid full Commit line")
    return text, match.group(1)


def ensure_report_value(report_text, field, expected, label):
    lines = re.findall(rf"(?m)^{re.escape(field)}:.*$", report_text)
    if lines != [f"{field}: {expected}"]:
        raise EvidenceError(f"{label} report {field} does not match its evidence")


def validate_reach(repo, extractor, report_path):
    report = read_json(report_path, "reach gate report")
    if not isinstance(report, dict) or report.get("result") != "PASS":
        raise EvidenceError("reach gate report is not PASS")
    if report.get("source_commit") != report.get("commit"):
        raise EvidenceError("reach report source_commit must match its repository HEAD commit")
    ensure_ancestor(repo, report.get("source_commit"))
    validate_binary_identity(report.get("tested_binary"), "reach tested code-kb binary")
    recorded = validate_snapshot(report.get("evidence"), "reach evidence")
    current = capture(repo, extractor)
    changes = snapshot_changes(recorded, current)
    if changes:
        raise EvidenceError(
            "reach PASS does not cover current inputs: " + ", ".join(changes)
            + ". Rerun scripts/reach_gate.py run on this source."
        )
    config = config_for(repo)
    ensure_inputs_clean(repo, config)
    return current["fingerprint"]


def dogfood_session_directory(path, evidence):
    evidence_root = Path(path).expanduser().resolve().parent
    session_name = evidence.get("session_dir")
    if not isinstance(session_name, str) or not re.fullmatch(r"[A-Za-z0-9._-]+", session_name) or session_name in {".", ".."}:
        raise EvidenceError("malformed dogfood evidence: invalid session directory")
    session_dir = (evidence_root / session_name).resolve()
    if not session_dir.is_relative_to(evidence_root) or session_dir == evidence_root:
        raise EvidenceError("malformed dogfood evidence: session directory is outside the evidence directory")
    return evidence_root, session_dir, session_name


def validate_session_transcripts(path, evidence):
    evidence_root, session_dir, session_name = dogfood_session_directory(path, evidence)
    sessions = evidence.get("sessions")
    if not isinstance(sessions, dict) or set(sessions) != {"claude", "codex"}:
        raise EvidenceError("malformed dogfood evidence: both session transcripts must be finalized")
    for harness in ("claude", "codex"):
        record = sessions[harness]
        expected = f"{session_name}/{harness}.md"
        if not isinstance(record, dict) or record.get("path") != expected:
            raise EvidenceError(f"malformed dogfood evidence: {harness} transcript path is outside its round")
        digest = record.get("sha256")
        if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise EvidenceError(f"malformed dogfood evidence: invalid {harness} transcript SHA-256")
        transcript = (evidence_root / expected).resolve()
        if not transcript.is_relative_to(session_dir) or not transcript.is_relative_to(evidence_root):
            raise EvidenceError(f"malformed dogfood evidence: {harness} transcript is outside the evidence directory")
        try:
            content = transcript.read_bytes()
        except OSError as error:
            raise EvidenceError(f"missing {harness} session transcript: {transcript}") from error
        if not content.strip():
            raise EvidenceError(f"empty {harness} session transcript: {transcript}")
        if hashlib.sha256(content).hexdigest() != digest:
            raise EvidenceError(f"changed {harness} session transcript: {transcript}")


def load_dogfood_evidence(path, require_sessions=True):
    evidence = read_json(path, "dogfood evidence")
    if not isinstance(evidence, dict):
        raise EvidenceError("malformed dogfood evidence: expected an object")
    version = evidence.get("schema")
    if not isinstance(version, int) or version != SCHEMA:
        reason = "future" if isinstance(version, int) and version > SCHEMA else "unsupported or missing"
        raise EvidenceError(f"{reason} dogfood evidence schema {version!r}; supported schema is {SCHEMA}")
    if evidence.get("kind") != "dogfood":
        raise EvidenceError("malformed dogfood evidence: expected kind 'dogfood'")
    round_commit = evidence.get("round_commit")
    if not isinstance(round_commit, str) or not re.fullmatch(r"[0-9a-f]{40}", round_commit):
        raise EvidenceError("malformed dogfood evidence: invalid round commit")
    if evidence.get("source_commit") != round_commit:
        raise EvidenceError("malformed dogfood evidence: source commit does not match the full-round commit")
    validate_binary_identity(evidence.get("tested_binary"), "dogfood tested code-kb binary")
    validate_snapshot(evidence.get("snapshot"), "dogfood snapshot")
    dogfood_session_directory(path, evidence)
    if require_sessions:
        validate_session_transcripts(path, evidence)
    if not isinstance(evidence.get("fixes"), list):
        raise EvidenceError("malformed dogfood evidence: fixes must be a list")
    return evidence


def start_dogfood(repo, extractor, output, binary, session_dir):
    root = Path(repo).resolve()
    config = config_for(root)
    ensure_inputs_clean(root, config)
    evidence_path = Path(output).expanduser().resolve()
    session_path = Path(session_dir).expanduser()
    if not session_path.is_absolute():
        raise EvidenceError("dogfood session directory must be absolute")
    session_path = session_path.resolve()
    try:
        session_name = session_path.relative_to(evidence_path.parent).as_posix()
    except ValueError as error:
        raise EvidenceError("dogfood session directory must be inside the evidence directory") from error
    if not re.fullmatch(r"[A-Za-z0-9._-]+", session_name) or session_name in {".", ".."}:
        raise EvidenceError("dogfood session directory must be one round folder inside the evidence directory")
    if not session_path.is_dir():
        raise EvidenceError(f"missing dogfood session directory: {session_path}")
    round_commit = commit_id(root)
    evidence = {
        "schema": SCHEMA,
        "kind": "dogfood",
        "round_commit": round_commit,
        "source_commit": round_commit,
        "tested_binary": binary_identity(binary),
        "snapshot": capture(root, extractor),
        "session_dir": session_name,
        "fixes": [],
    }
    evidence_path.parent.mkdir(parents=True, exist_ok=True)
    evidence_path.write_text(json.dumps(evidence, indent=2) + "\n")
    return evidence


def finalize_dogfood(evidence_path):
    evidence = load_dogfood_evidence(evidence_path, require_sessions=False)
    if "sessions" in evidence:
        raise EvidenceError("dogfood sessions are already finalized")
    evidence_root, session_dir, session_name = dogfood_session_directory(evidence_path, evidence)
    sessions = {}
    for harness in ("claude", "codex"):
        transcript = session_dir / f"{harness}.md"
        if not transcript.is_file():
            raise EvidenceError(f"missing {harness} session transcript: {transcript}")
        resolved = transcript.resolve()
        if not resolved.is_relative_to(session_dir) or not resolved.is_relative_to(evidence_root):
            raise EvidenceError(f"{harness} session transcript is outside the evidence directory: {transcript}")
        content = resolved.read_bytes()
        if not content.strip():
            raise EvidenceError(f"empty {harness} session transcript: {transcript}")
        sessions[harness] = {
            "path": f"{session_name}/{harness}.md",
            "sha256": hashlib.sha256(content).hexdigest(),
        }
    evidence["sessions"] = sessions
    write_json(evidence_path, evidence)
    return evidence


def command_argv(value, label):
    try:
        args = json.loads(value)
    except json.JSONDecodeError as error:
        raise EvidenceError(f"{label} must be a JSON argument array: {error}") from error
    if not isinstance(args, list) or not args or not all(isinstance(arg, str) and arg for arg in args):
        raise EvidenceError(f"{label} must be a non-empty JSON string array")
    return args


def validate_code_kb_call(args):
    if not args:
        raise EvidenceError("exact call must invoke the code-kb CLI")
    binary = Path(args[0]).expanduser()
    if not binary.is_absolute() or binary.name.lower() not in {"code-kb", "code-kb.exe"}:
        raise EvidenceError("exact call must invoke the code-kb CLI")
    root_values = []
    db_values = []
    command = None
    index = 1
    while index < len(args):
        argument = args[index]
        if argument in {"--help", "-h", "--version", "-V"}:
            raise EvidenceError("exact call must run a query/report command, not help or version")
        if argument in {"--verbose", "-v"}:
            raise EvidenceError("exact call must omit verbose logging")
        if argument == "--json":
            index += 1
            continue
        if argument in {"--root", "--db"}:
            if index + 1 >= len(args):
                raise EvidenceError(f"exact code-kb call has no value after {argument}")
            value = args[index + 1]
            if argument == "--root":
                root_values.append(value)
            else:
                db_values.append(value)
            index += 2
            continue
        if argument.startswith("--root="):
            root_values.append(argument.partition("=")[2])
            index += 1
            continue
        if argument.startswith("--db="):
            db_values.append(argument.partition("=")[2])
            index += 1
            continue
        if command is None:
            if argument.startswith("-"):
                raise EvidenceError(f"exact code-kb call has unsupported global option {argument}")
            command = argument
        index += 1
    if len(root_values) != 1 or not Path(root_values[0]).is_absolute():
        raise EvidenceError("exact code-kb call must include an absolute --root")
    if command not in EXACT_CALL_COMMANDS:
        raise EvidenceError("exact call must run a supported code-kb query/report command")
    if len(db_values) > 1:
        raise EvidenceError("exact code-kb call must not repeat --db")
    project_root = Path(root_values[0]).expanduser().resolve()
    if db_values:
        db = Path(db_values[0]).expanduser()
        if not db.is_absolute():
            raise EvidenceError("exact code-kb --db path must be absolute and inside --root")
        db = db.resolve()
        if not db.is_relative_to(project_root) or not db.is_file():
            raise EvidenceError("exact code-kb --db path must be an existing index inside --root")
    return {"binary_path": str(binary.resolve()), "project_root": str(project_root), "command": command}


def run_evidence_command(repo, args, label):
    try:
        result = subprocess.run(
            args,
            cwd=repo,
            capture_output=True,
            text=True,
            check=False,
            timeout=COMMAND_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise EvidenceError(f"{label} could not be reproduced: {error}") from error
    output = (result.stdout or "") + (result.stderr or "")
    if result.returncode:
        excerpt = output[-MAX_OUTPUT_CHARS:]
        raise EvidenceError(f"{label} exited {result.returncode}: {excerpt}")
    if not output.strip():
        raise EvidenceError(f"{label} succeeded without producing inspectable output")
    return {
        "argv": args,
        "output_sha256": hashlib.sha256(output.encode()).hexdigest(),
        "output_excerpt": output[:MAX_OUTPUT_CHARS],
    }


def fix_report_line(commit, defect, tested_binary):
    return (
        f"- [fixed {commit}] {defect}: regression and exact call were rerun locally; tested binary "
        f"{tested_binary['path']} SHA-256 {tested_binary['sha256']}; outputs are recorded in evidence.json."
    )


def record_fix(repo, extractor, evidence_path, report_path, commit, defect, test_command, exact_call):
    root = Path(repo).resolve()
    config = config_for(root)
    ensure_inputs_clean(root, config)
    evidence = load_dogfood_evidence(evidence_path)
    base = evidence["round_commit"]
    ensure_ancestor(root, base)
    commit = commit_id(root, commit)
    ensure_ancestor(root, commit)
    if commit == base:
        raise EvidenceError("a dogfood fix must be a commit after the full round")
    head = commit_id(root)
    if commit != head:
        raise EvidenceError(f"record-fix commit {commit} must equal current HEAD {head}")
    if not isinstance(defect, str) or not defect.strip() or len(defect) > 300 or "\n" in defect or "\r" in defect:
        raise EvidenceError("defect must be a non-empty single line of at most 300 characters")
    if test_command not in config["test_commands"]:
        raise EvidenceError("test command is not listed in scripts/release-evidence-inputs.json")
    test_argv = shlex.split(test_command)
    if not test_argv:
        raise EvidenceError("test command is empty")
    exact_argv = command_argv(exact_call, "exact call")
    call_info = validate_code_kb_call(exact_argv)
    changed = [path for path in commit_paths(root, commit) if input_path_matches(path, config)]
    baseline_inputs = evidence["snapshot"]["inputs"]
    current = capture(root, extractor)
    current_inputs = current["inputs"]
    relevant = [
        path
        for path in changed
        if baseline_inputs.get(path) != current_inputs.get(path)
    ]
    if not relevant:
        raise EvidenceError(f"commit {commit} has no changed release evidence inputs")
    latest, _ = latest_input_commits(root, base, config)
    stale = [path for path in relevant if latest.get(path) != commit]
    if stale:
        raise EvidenceError(f"fix commit {commit} is stale; a later commit changed {', '.join(stale)}")
    report = Path(report_path)
    text = report.read_text()
    _, report_commit = ensure_pass_report(report, "dogfood")
    if report_commit != base:
        raise EvidenceError("dogfood report Commit does not match its captured full-round commit")
    tested_snapshot = current
    if tested_snapshot["extractor"] != evidence["snapshot"]["extractor"]:
        raise EvidenceError("dogfood extractor identity or level changed; run one full dogfood round")
    test_result = run_evidence_command(root, test_argv, "regression test command")
    after_test = capture(root, extractor)
    if commit_id(root) != commit or after_test["fingerprint"] != tested_snapshot["fingerprint"]:
        raise EvidenceError("source inputs changed while running the regression test command")
    tested_binary = binary_identity(call_info["binary_path"])
    call_result = run_evidence_command(root, exact_argv, "exact call")
    after_call = capture(root, extractor)
    binary_after_call = binary_identity(call_info["binary_path"])
    if commit_id(root) != commit or after_call["fingerprint"] != tested_snapshot["fingerprint"]:
        raise EvidenceError("source inputs changed while running the exact call")
    if binary_after_call != tested_binary:
        raise EvidenceError("tested code-kb binary changed while running the exact call")
    evidence["fixes"].append(
        {
            "commit": commit,
            "source_commit": commit,
            "snapshot": tested_snapshot,
            "tested_binary": tested_binary,
            "paths": relevant,
            "defect": defect,
            "test_command": test_command,
            "test_output_sha256": test_result["output_sha256"],
            "test_output_excerpt": test_result["output_excerpt"],
            "exact_call": call_result["argv"],
            "exact_call_output_sha256": call_result["output_sha256"],
            "exact_call_output_excerpt": call_result["output_excerpt"],
        }
    )
    marker = "## Result: PASS"
    evidence_line = fix_report_line(commit, defect, tested_binary) + "\n\n"
    if marker not in text:
        raise EvidenceError("dogfood report has no PASS result line")
    existing_line = re.compile(rf"(?m)^- \[fixed {re.escape(commit)}\] {re.escape(defect)}:.*$")
    if existing_line.search(text):
        text = existing_line.sub(evidence_line.rstrip(), text, count=1)
    else:
        text = text.replace(marker, evidence_line + marker, 1)
    report.write_text(text)
    Path(evidence_path).write_text(json.dumps(evidence, indent=2) + "\n")
    return evidence


def validate_fix_record(fix, report_text, base, latest, paths_by_commit, config, current_inputs, round_extractor):
    if not isinstance(fix, dict):
        raise EvidenceError("malformed dogfood fix evidence: each fix must be an object")
    commit = fix.get("commit")
    if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise EvidenceError("malformed dogfood fix evidence: invalid commit")
    if not isinstance(fix.get("paths"), list) or not fix["paths"]:
        raise EvidenceError(f"malformed dogfood fix evidence for {commit}: no changed paths")
    if not isinstance(fix.get("defect"), str) or not fix["defect"].strip():
        raise EvidenceError(f"malformed dogfood fix evidence for {commit}: no defect description")
    if not isinstance(fix.get("test_command"), str) or fix["test_command"] not in config["test_commands"]:
        raise EvidenceError(f"malformed dogfood fix evidence for {commit}: no regression command")
    if fix.get("source_commit") != commit:
        raise EvidenceError(f"malformed dogfood fix evidence for {commit}: tested source commit does not match")
    snapshot = validate_snapshot(fix.get("snapshot"), f"dogfood fix snapshot for {commit}")
    if snapshot["extractor"] != round_extractor:
        raise EvidenceError(f"dogfood fix {commit} used another extractor identity or level")
    tested_binary = validate_binary_identity(
        fix.get("tested_binary"), f"dogfood fix tested code-kb binary for {commit}"
    )
    if not re.fullmatch(r"[0-9a-f]{64}", str(fix.get("test_output_sha256", ""))) or not isinstance(fix.get("test_output_excerpt"), str) or not fix["test_output_excerpt"].strip():
        raise EvidenceError(f"malformed dogfood fix evidence for {commit}: no regression output")
    call = fix.get("exact_call")
    if not isinstance(call, list) or not call or not all(isinstance(arg, str) and arg for arg in call):
        raise EvidenceError(f"malformed dogfood fix evidence for {commit}: no exact call")
    call_info = validate_code_kb_call(call)
    if tested_binary["path"] != call_info["binary_path"]:
        raise EvidenceError(f"dogfood fix {commit} tested-binary path does not match its exact call")
    if not re.fullmatch(r"[0-9a-f]{64}", str(fix.get("exact_call_output_sha256", ""))) or not isinstance(fix.get("exact_call_output_excerpt"), str) or not fix["exact_call_output_excerpt"].strip():
        raise EvidenceError(f"malformed dogfood fix evidence for {commit}: no exact-call output")
    expected_line = fix_report_line(commit, fix["defect"], tested_binary)
    if report_text.splitlines().count(expected_line) != 1:
        raise EvidenceError(f"dogfood report has no [fixed {commit}] line for {fix['defect']!r}")
    if commit not in paths_by_commit:
        raise EvidenceError(f"dogfood fix commit {commit} is not after the full round at {base}")
    commit_paths_set = set(paths_by_commit[commit])
    for path in fix["paths"]:
        if not isinstance(path, str) or path not in commit_paths_set:
            raise EvidenceError(f"dogfood fix {commit} claims a path it did not change: {path!r}")
        if latest.get(path) != commit:
            raise EvidenceError(f"dogfood fix {commit} is stale; a later commit changed {path}")
        if snapshot["inputs"].get(path) != current_inputs.get(path):
            raise EvidenceError(f"dogfood fix {commit} snapshot does not match its latest changed input {path}")


def validate_dogfood(repo, extractor, evidence_path, report_path):
    root = Path(repo).resolve()
    evidence = load_dogfood_evidence(evidence_path)
    base = evidence.get("round_commit")
    ensure_ancestor(root, base)
    report_text, report_commit = ensure_pass_report(report_path, "dogfood")
    if report_commit != base:
        raise EvidenceError("dogfood report Commit does not match its captured full-round commit")
    fingerprint = evidence["snapshot"]["fingerprint"]
    ensure_report_value(report_text, "Source commit", base, "dogfood")
    ensure_report_value(report_text, "Input fingerprint", fingerprint, "dogfood")
    tested_binary = evidence["tested_binary"]
    ensure_report_value(report_text, "Tested binary", tested_binary["path"], "dogfood")
    ensure_report_value(report_text, "Tested binary SHA-256", tested_binary["sha256"], "dogfood")
    sessions = evidence["sessions"]
    ensure_report_value(
        report_text,
        "Sessions",
        f"{sessions['claude']['path']}, {sessions['codex']['path']}",
        "dogfood",
    )
    for harness in ("claude", "codex"):
        record = sessions[harness]
        label = harness.title() + " transcript"
        ensure_report_value(report_text, label, record["path"], "dogfood")
        ensure_report_value(report_text, label + " SHA-256", record["sha256"], "dogfood")
    config = config_for(root)
    ensure_inputs_clean(root, config)
    current = capture(root, extractor)
    baseline = evidence["snapshot"]
    if baseline["extractor"] != current["extractor"]:
        raise EvidenceError("dogfood extractor identity or level changed; run one full dogfood round")
    changes = snapshot_changes(baseline, current)
    if not changes:
        return current["fingerprint"]
    changed_paths = [path for path in changes if path in baseline["inputs"] or path in current["inputs"]]
    if not changed_paths:
        raise EvidenceError("dogfood inputs changed without a source fix; run one full dogfood round")
    latest, paths_by_commit = latest_input_commits(root, base, config)
    fixes = evidence["fixes"]
    by_commit = {}
    for fix in fixes:
        validate_fix_record(
            fix,
            report_text,
            base,
            latest,
            paths_by_commit,
            config,
            current["inputs"],
            baseline["extractor"],
        )
        by_commit.setdefault(fix["commit"], set()).update(fix["paths"])
    missing = {}
    for path in changed_paths:
        commit = latest.get(path)
        if commit is None:
            raise EvidenceError(f"changed evidence input {path} has no committed fix after the dogfood round")
        fix = by_commit.get(commit)
        if fix is None or path not in fix:
            missing.setdefault(commit, []).append(path)
    if missing:
        detail = "; ".join(
            f"{commit[:12]} ({', '.join(sorted(paths))})" for commit, paths in sorted(missing.items())
        )
        raise EvidenceError(
            f"later source changes lack regression and exact-call reproduction evidence: {detail}. "
            "Run the regression command and exact CLI call, then record them with scripts/release_evidence.py record-fix."
        )
    return current["fingerprint"]


def write_json(path, value):
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(value, indent=2) + "\n")


def render_dogfood_report(evidence_path, output, code_kb_version, julie_version, round_name):
    evidence = load_dogfood_evidence(evidence_path)
    values = {
        "code-kb version": code_kb_version,
        "julie-extract version": julie_version,
        "round": round_name,
    }
    for label, value in values.items():
        if not isinstance(value, str) or not value.strip() or "\n" in value or "\r" in value:
            raise EvidenceError(f"{label} must be a non-empty single line")
    source_commit = evidence["source_commit"]
    tested_binary = evidence["tested_binary"]
    if round_name != evidence["session_dir"]:
        raise EvidenceError("round name does not match the finalized dogfood session directory")
    sessions = evidence["sessions"]
    lines = [
        "# Release dogfood report",
        "",
        f"Commit: {source_commit}",
        f"Source commit: {source_commit}",
        f"New: code-kb {code_kb_version}, julie-extract {julie_version}",
        f"Sessions: {sessions['claude']['path']}, {sessions['codex']['path']}",
        f"Claude transcript: {sessions['claude']['path']}",
        f"Claude transcript SHA-256: {sessions['claude']['sha256']}",
        f"Codex transcript: {sessions['codex']['path']}",
        f"Codex transcript SHA-256: {sessions['codex']['sha256']}",
        f"Input fingerprint: {evidence['snapshot']['fingerprint']}",
        f"Tested binary: {tested_binary['path']}",
        f"Tested binary SHA-256: {tested_binary['sha256']}",
        "Commit identifies source HEAD; it does not prove how the tested binary was built.",
        "Evidence: target/release-dogfood/evidence.json",
        "",
        "## Defects",
        "",
        "Check every defect that a session reports against the source. List each one here as:",
        "- [open] <defect>",
        "- [fixed <commit>] <defect>: a wrong answer, fixed with a regression test and a rerun of the call",
        "- [gap] <defect>: a true but incomplete answer, a heuristic miss, or a feature request",
        "- [not a defect] <defect>: <why the output is correct>",
        '- [deferred: "<the user\'s own words>"] <defect>',
        "",
        "For each later source fix, commit it, then record a passing regression test command and the exact",
        "CLI call that the session reported. Use a command from scripts/release-evidence-inputs.json:",
        "`python3 scripts/release_evidence.py record-fix --repo . --evidence target/release-dogfood/evidence.json --report target/release-dogfood/report.md --commit <fix-commit> --defect \"<defect>\" --test-command \"<configured command>\" --exact-call-json '[\"/absolute/path/to/code-kb\", \"--root\", \"<project-root>\", \"<query-command>\", \"<arguments>\"]'`.",
        "The command runs both local checks and records their output. Extractor identity or level changes",
        "need one new full round. A source fix does not need another paid round when its exact-call and",
        "regression evidence is recorded (docs/RELEASING.md, step 7).",
        "",
        "## Result: PENDING",
        "",
    ]
    target = Path(output)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("\n".join(lines))
    return target


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    capture_parser = commands.add_parser("capture", help="capture current semantic evaluation inputs")
    capture_parser.add_argument("--repo", required=True)
    capture_parser.add_argument("--extractor")
    capture_parser.add_argument("--out", required=True)
    start = commands.add_parser("start-dogfood", help="capture the inputs for the single full dogfood round")
    start.add_argument("--repo", required=True)
    start.add_argument("--extractor")
    start.add_argument("--binary", required=True, help="code-kb binary used by the dogfood session")
    start.add_argument("--session-dir", required=True, help="round output directory for both session transcripts")
    start.add_argument("--out", required=True)
    finalize = commands.add_parser("finalize-dogfood", help="hash the two completed dogfood session transcripts")
    finalize.add_argument("--evidence", required=True)
    render = commands.add_parser("render-dogfood-report", help="write the local dogfood report template")
    render.add_argument("--evidence", required=True)
    render.add_argument("--out", required=True)
    render.add_argument("--code-kb-version", required=True)
    render.add_argument("--julie-version", required=True)
    render.add_argument("--round", required=True)
    reach = commands.add_parser("validate-reach", help="validate a reach PASS against current inputs")
    reach.add_argument("--repo", required=True)
    reach.add_argument("--extractor")
    reach.add_argument("--report", required=True)
    fix = commands.add_parser("record-fix", help="run and record local evidence for a post-round fix")
    fix.add_argument("--repo", required=True)
    fix.add_argument("--extractor")
    fix.add_argument("--evidence", required=True)
    fix.add_argument("--report", required=True)
    fix.add_argument("--commit", required=True)
    fix.add_argument("--defect", required=True)
    fix.add_argument("--test-command", required=True)
    fix.add_argument("--exact-call-json", required=True)
    dogfood = commands.add_parser("validate-dogfood", help="validate one dogfood round and its local fixes")
    dogfood.add_argument("--repo", required=True)
    dogfood.add_argument("--extractor")
    dogfood.add_argument("--evidence", required=True)
    dogfood.add_argument("--report", required=True)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        if args.command == "capture":
            result = capture(args.repo, args.extractor)
            write_json(args.out, result)
            print(f"Captured {result['fingerprint']} ({len(result['inputs'])} inputs)")
        elif args.command == "start-dogfood":
            result = start_dogfood(args.repo, args.extractor, args.out, args.binary, args.session_dir)
            print(f"Captured dogfood round {result['round_commit']} at {result['snapshot']['fingerprint']}")
        elif args.command == "finalize-dogfood":
            result = finalize_dogfood(args.evidence)
            print(f"Finalized dogfood session transcripts for {result['session_dir']}")
        elif args.command == "render-dogfood-report":
            report = render_dogfood_report(
                args.evidence,
                args.out,
                args.code_kb_version,
                args.julie_version,
                args.round,
            )
            print(f"Wrote dogfood report template: {report}")
        elif args.command == "validate-reach":
            fingerprint = validate_reach(args.repo, args.extractor, args.report)
            print(f"Reach evidence matches current inputs: {fingerprint}")
        elif args.command == "record-fix":
            exact_call = command_argv(args.exact_call_json, "exact call")
            evidence = record_fix(
                args.repo,
                args.extractor,
                args.evidence,
                args.report,
                args.commit,
                args.defect,
                args.test_command,
                json.dumps(exact_call),
            )
            print(f"Recorded local reproduction for {evidence['fixes'][-1]['commit']}")
        else:
            fingerprint = validate_dogfood(args.repo, args.extractor, args.evidence, args.report)
            print(f"Dogfood evidence covers current inputs: {fingerprint}")
        return 0
    except (EvidenceError, OSError, subprocess.TimeoutExpired) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
