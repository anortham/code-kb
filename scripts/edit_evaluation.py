"""Prepare, validate, execute, replay, and summarize completed-edit cases."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import posixpath
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path, PurePosixPath
from typing import Any, Mapping, Sequence

try:
    from . import comparison_metrics, edit_sandbox, edit_snapshot
except ImportError:
    import comparison_metrics
    import edit_sandbox
    import edit_snapshot


SCHEMA = 1
REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORPUS = REPO_ROOT / "evals/edit-2026-10-27/corpus.json"
CASE_KINDS = {"regression", "feature", "migration"}
ARMS = {"native", "code-kb"}
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
GIT_OBJECT_ID = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
MAX_JSON_BYTES = 4 * 1024 * 1024
MAX_ARCHIVE_FILE_BYTES = 512 * 1024 * 1024
MAX_TRACE_BYTES = 64 * 1024 * 1024
MAX_TRACE_EVENTS = 200_000
ANSI_ESCAPE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07]*(?:\x07|\x1b\\)|\x1b[@-_]", re.DOTALL)


class EvaluationError(ValueError):
    """Raised when a case, source, run, or manifest violates the protocol."""


def _read_json(path: Path) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_JSON_BYTES:
        raise EvaluationError(f"expected a bounded regular JSON file: {path}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise EvaluationError(f"invalid JSON file {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise EvaluationError(f"expected an object in {path}")
    return value


def _write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    data = json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    temporary.write_text(data, encoding="utf-8")
    os.replace(temporary, path)


def _relative(value: Any, label: str, *, allow_dot: bool = False) -> str:
    if not isinstance(value, str) or not value or "\\" in value or "\x00" in value:
        raise EvaluationError(f"{label} must be a relative forward-slash path")
    if allow_dot and value == ".":
        return value
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in value.split("/")):
        raise EvaluationError(f"{label} escapes its root: {value!r}")
    return path.as_posix()


def _paths_overlap(left: str, right: str) -> bool:
    return left == right or left.startswith(right + "/") or right.startswith(left + "/")


def _case_file(case_dir: Path, relative: str, label: str) -> Path:
    path = case_dir
    for part in PurePosixPath(_relative(relative, label)).parts:
        path = path / part
        if path.is_symlink():
            raise EvaluationError(f"{label} cannot traverse a symlink: {relative}")
    if not path.is_file():
        raise EvaluationError(f"{label} must name an existing regular file: {relative}")
    return path


def _command(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise EvaluationError(f"{label} must be an object")
    command_id = value.get("id")
    argv = value.get("argv")
    timeout = value.get("timeout_seconds")
    if not isinstance(command_id, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,99}", command_id):
        raise EvaluationError(f"{label}.id is invalid")
    if not isinstance(argv, list) or not argv or any(not isinstance(arg, str) or not arg or "\x00" in arg for arg in argv):
        raise EvaluationError(f"{label}.argv must be a non-empty string array")
    cwd = _relative(value.get("cwd", "."), f"{label}.cwd", allow_dot=True)
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not 1 <= timeout <= 3600:
        raise EvaluationError(f"{label}.timeout_seconds must be between 1 and 3600")
    markers = value.get("expected_output", [])
    if not isinstance(markers, list) or any(not isinstance(marker, str) or not marker.strip() for marker in markers):
        raise EvaluationError(f"{label}.expected_output must be a string array")
    return {"id": command_id, "argv": list(argv), "cwd": cwd, "timeout_seconds": timeout, "expected_output": markers}


def validate_case(case_dir: str | Path, *, corpus_path: str | Path = DEFAULT_CORPUS, verify_assets: bool = True) -> dict[str, Any]:
    """Validate one case contract and return its parsed JSON object."""
    directory = Path(case_dir).resolve(strict=True)
    if not directory.is_dir():
        raise EvaluationError(f"case path is not a directory: {directory}")
    case = _read_json(directory / "case.json")
    if case.get("schema") != SCHEMA:
        raise EvaluationError("case schema must be 1")
    for key in ("id", "repo_id"):
        if not isinstance(case.get(key), str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,99}", case[key]):
            raise EvaluationError(f"case {key} is invalid")
    if case.get("task_kind") not in CASE_KINDS:
        raise EvaluationError("task_kind must be regression, feature, or migration")

    for key in ("request", "seed_patch", "reference_patch"):
        case[key] = _relative(case.get(key), f"case.{key}")
    hidden_files = case.get("hidden_files")
    if not isinstance(hidden_files, list) or not hidden_files:
        raise EvaluationError("hidden_files must be a non-empty array")
    hidden_destinations = set()
    for index, item in enumerate(hidden_files):
        if not isinstance(item, dict):
            raise EvaluationError(f"hidden_files[{index}] must be an object")
        destination = _relative(item.get("path"), f"hidden_files[{index}].path")
        source = _relative(item.get("source"), f"hidden_files[{index}].source")
        if destination in hidden_destinations:
            raise EvaluationError(f"duplicate hidden test destination: {destination}")
        hidden_destinations.add(destination)
        item["path"], item["source"] = destination, source

    for key in ("protected_paths", "generated_roots"):
        values = case.get(key)
        if not isinstance(values, list) or any(not isinstance(item, str) for item in values):
            raise EvaluationError(f"{key} must be an array of relative paths")
        case[key] = [_relative(item, f"{key} path") for item in values]
        if len(case[key]) != len(set(case[key])):
            raise EvaluationError(f"{key} contains duplicate paths")
    if ".git" not in case["protected_paths"]:
        raise EvaluationError(".git must be a protected path")
    if set(hidden_destinations) - set(case["protected_paths"]):
        raise EvaluationError("every hidden destination must reserve a protected path")
    if set(hidden_destinations) & set(case["generated_roots"]):
        raise EvaluationError("hidden test paths cannot be generated roots")
    if any(
        generated == protected or generated.startswith(protected + "/") or protected.startswith(generated + "/")
        for generated in case["generated_roots"]
        for protected in case["protected_paths"]
    ):
        raise EvaluationError("generated roots overlap protected paths")

    production_files = case.get("production_files")
    if not isinstance(production_files, list) or len(production_files) < 2:
        raise EvaluationError("production_files must name at least two production paths")
    case["production_files"] = [_relative(item, "production file") for item in production_files]
    if len(case["production_files"]) != len(set(case["production_files"])):
        raise EvaluationError("production_files contains duplicate paths")

    reference_test_files = case.get("reference_test_files", [])
    if not isinstance(reference_test_files, list) or any(not isinstance(item, str) for item in reference_test_files):
        raise EvaluationError("reference_test_files must be an array of relative paths")
    reference_test_files = [_relative(item, "reference test file") for item in reference_test_files]
    if len(reference_test_files) != len(set(reference_test_files)):
        raise EvaluationError("reference_test_files contains duplicate paths")
    for reference_path in reference_test_files:
        if _paths_overlap(reference_path, ".git"):
            raise EvaluationError(f"reference test path overlaps protected Git metadata: {reference_path}")
        if any(_paths_overlap(reference_path, production_path) for production_path in case["production_files"]):
            raise EvaluationError(f"reference test path overlaps a production file: {reference_path}")
        if any(_paths_overlap(reference_path, hidden_path) for hidden_path in hidden_destinations):
            raise EvaluationError(f"reference test path overlaps a hidden destination: {reference_path}")
        if any(_paths_overlap(reference_path, generated_path) for generated_path in case["generated_roots"]):
            raise EvaluationError(f"reference test path overlaps a generated root: {reference_path}")
        if any(_paths_overlap(reference_path, protected_path) for protected_path in case["protected_paths"]):
            raise EvaluationError(f"reference test path overlaps a protected path: {reference_path}")
    for index, reference_path in enumerate(reference_test_files):
        if any(_paths_overlap(reference_path, other) for other in reference_test_files[index + 1:]):
            raise EvaluationError("reference test files overlap")
    case["reference_test_files"] = reference_test_files

    for key in ("public_commands", "hidden_commands"):
        values = case.get(key)
        if not isinstance(values, list) or not values:
            raise EvaluationError(f"{key} must be a non-empty array")
        case[key] = [_command(command, f"{key}[{index}]") for index, command in enumerate(values)]
        if any(not command["expected_output"] for command in case[key]):
            raise EvaluationError(f"every {key} command needs at least one literal expected_output marker")
        ids = [command["id"] for command in case[key]]
        if len(ids) != len(set(ids)):
            raise EvaluationError(f"{key} command ids must be unique")

    all_command_ids = {item["id"] for item in case["public_commands"] + case["hidden_commands"]}
    for key in ("setup_commands", "runtime_setup_commands"):
        values = case.get(key, [])
        if not isinstance(values, list):
            raise EvaluationError(f"{key} must be an array")
        case[key] = [_command(command, f"{key}[{index}]") for index, command in enumerate(values)]
        ids = [command["id"] for command in case[key]]
        if len(ids) != len(set(ids)) or all_command_ids.intersection(ids):
            raise EvaluationError(f"{key} command ids must be unique across the case")

    for key in ("dependency_inputs", "tool_inputs"):
        values = case.get(key, [])
        if not isinstance(values, list) or any(not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,99}", name) for name in values):
            raise EvaluationError(f"{key} must be an array of mount names")
        if len(values) != len(set(values)):
            raise EvaluationError(f"{key} contains duplicate names")
        case[key] = values
    dependency_outputs = case.get("dependency_output_paths", {})
    if not isinstance(dependency_outputs, dict) or set(dependency_outputs) - set(case["dependency_inputs"]):
        raise EvaluationError("dependency_output_paths must map declared dependency inputs only")
    case["dependency_output_paths"] = {
        name: _relative(value, f"dependency_output_paths.{name}")
        for name, value in dependency_outputs.items()
    }

    expected_failure = case.get("expected_seed_failure")
    if not isinstance(expected_failure, dict):
        raise EvaluationError("expected_seed_failure must be an object")
    if expected_failure.get("command_id") not in {item["id"] for item in case["hidden_commands"]}:
        raise EvaluationError("expected_seed_failure.command_id must name a hidden command")
    codes = expected_failure.get("exit_codes")
    if not isinstance(codes, list) or not codes or any(isinstance(code, bool) or not isinstance(code, int) or code == 0 for code in codes):
        raise EvaluationError("expected_seed_failure.exit_codes must contain non-zero integers")
    diagnostic = expected_failure.get("diagnostic_contains")
    if not isinstance(diagnostic, str) or not diagnostic:
        raise EvaluationError("expected_seed_failure.diagnostic_contains must be a non-empty literal")

    corpus = _read_json(Path(corpus_path).resolve(strict=True))
    repositories = corpus.get("repositories")
    if not isinstance(repositories, list):
        raise EvaluationError("corpus repositories must be an array")
    matching = [repo for repo in repositories if isinstance(repo, dict) and repo.get("id") == case["repo_id"]]
    if len(matching) != 1:
        raise EvaluationError(f"case repository is not uniquely pinned in corpus.json: {case['repo_id']}")
    case["source_pin"] = matching[0]["commit"]
    case["source_url"] = matching[0]["url"]

    if verify_assets:
        for key in ("request", "seed_patch", "reference_patch"):
            _case_file(directory, case[key], key)
        for index, item in enumerate(hidden_files):
            _case_file(directory, item["source"], f"hidden_files[{index}].source")
    return case


def _git_env() -> dict[str, str]:
    environment = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_AUTHOR_NAME": "code-kb evaluation",
        "GIT_AUTHOR_EMAIL": "evaluation@localhost",
        "GIT_COMMITTER_NAME": "code-kb evaluation",
        "GIT_COMMITTER_EMAIL": "evaluation@localhost",
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
    }
    for key in ("HOME", "CODEX_HOME"):
        if key in os.environ:
            environment[key] = os.environ[key]
    return environment


def _git(repository: Path, args: Sequence[str]) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", *args],
        cwd=repository,
        env=_git_env(),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=120,
        check=False,
    )
    if completed.returncode:
        raise EvaluationError(f"git {' '.join(args)} failed ({completed.returncode}): {completed.stderr[-2000:]}")
    return completed


def _archive_entries(archive: Path) -> list[tuple[tarfile.TarInfo, str]]:
    entries = []
    seen = set()
    with tarfile.open(archive, mode="r:") as source:
        for member in source.getmembers():
            name = member.name.rstrip("/")
            if name in {"", "."} and member.isdir():
                continue
            path = PurePosixPath(name)
            if path.is_absolute() or "\\" in name or any(part in {"", ".", ".."} for part in name.split("/")):
                raise EvaluationError(f"source archive contains an unsafe path: {member.name!r}")
            normalized = path.as_posix()
            if normalized in seen:
                raise EvaluationError(f"source archive repeats a path: {normalized}")
            seen.add(normalized)
            if not (member.isdir() or member.isfile() or member.issym()):
                raise EvaluationError(f"source archive contains an unsupported entry: {normalized}")
            if member.isfile() and member.size > MAX_ARCHIVE_FILE_BYTES:
                raise EvaluationError(f"source archive file exceeds the limit: {normalized}")
            if member.issym():
                target = member.linkname
                if not target or target.startswith("/") or "\\" in target:
                    raise EvaluationError(f"source archive has an unsafe symbolic link: {normalized}")
                resolved = posixpath.normpath(posixpath.join(posixpath.dirname(normalized), target))
                if resolved == ".." or resolved.startswith("../"):
                    raise EvaluationError(f"source archive has an escaping symbolic link: {normalized}")
            entries.append((member, normalized))
    path_types = {path: info for info, path in entries}
    for _, path in entries:
        parts = PurePosixPath(path).parts
        for index in range(1, len(parts)):
            ancestor = PurePosixPath(*parts[:index]).as_posix()
            if ancestor in path_types and not path_types[ancestor].isdir():
                raise EvaluationError(f"source archive places an entry below a non-directory: {ancestor}")
    return entries


def _extract_archive(archive: Path, destination: Path) -> None:
    entries = _archive_entries(archive)
    destination.mkdir(parents=True, exist_ok=False)
    with tarfile.open(archive, mode="r:") as source:
        links = []
        for member, name in entries:
            target = destination.joinpath(*PurePosixPath(name).parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                target.chmod(0o755)
            elif member.issym():
                links.append((target, member.linkname))
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                stream = source.extractfile(member)
                if stream is None:
                    raise EvaluationError(f"source archive cannot read {name}")
                with stream, target.open("xb") as output:
                    shutil.copyfileobj(stream, output, 1024 * 1024)
                target.chmod(0o755 if member.mode & 0o111 else 0o644)
        for link_path, link_target in links:
            link_path.parent.mkdir(parents=True, exist_ok=True)
            os.symlink(link_target, link_path)


def export_source(repository: str | Path, commit: str, destination: str | Path) -> dict[str, Any]:
    """Export the tracked files from one exact commit without copying its Git history."""
    source = Path(repository).resolve(strict=True)
    output = Path(destination).resolve(strict=False)
    if source == output or source in output.parents or output in source.parents:
        raise EvaluationError("source export destination must be separate from its checkout")
    if output.exists() or output.is_symlink():
        raise EvaluationError(f"source export destination already exists: {output}")
    if not GIT_OBJECT_ID.fullmatch(commit):
        raise EvaluationError("source commit must be a full lowercase Git object id")
    actual = _git(source, ["rev-parse", "HEAD"]).stdout.strip()
    if actual != commit:
        raise EvaluationError(f"source checkout is at {actual}, expected pinned commit {commit}")
    with tempfile.TemporaryDirectory(prefix="edit-source-archive-") as archive_dir:
        archive = Path(archive_dir) / "source.tar"
        with archive.open("wb") as stream:
            completed = subprocess.run(
                ["git", "-c", "core.hooksPath=/dev/null", "archive", "--format=tar", commit],
                cwd=source,
                env=_git_env(),
                stdout=stream,
                stderr=subprocess.PIPE,
                timeout=180,
                check=False,
            )
        if completed.returncode:
            raise EvaluationError(f"git archive failed ({completed.returncode}): {completed.stderr.decode(errors='replace')[-2000:]}")
        _extract_archive(archive, output)
    return {"commit": commit, "tree_sha256": hash_tree(output), "file_count": sum(1 for path in output.rglob("*") if path.is_file() or path.is_symlink())}


def hash_tree(root: str | Path) -> str:
    """Hash regular files, directory names, links, and executable bits deterministically."""
    base = Path(root).resolve(strict=True)
    records = []
    for path in sorted(base.rglob("*"), key=lambda entry: entry.relative_to(base).as_posix()):
        relative = path.relative_to(base).as_posix()
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode):
            records.append({"path": relative, "kind": "symlink", "target": os.readlink(path)})
        elif stat.S_ISDIR(info.st_mode):
            records.append({"path": relative, "kind": "directory"})
        elif stat.S_ISREG(info.st_mode):
            digest = hashlib.sha256()
            with path.open("rb") as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
            records.append({"path": relative, "kind": "file", "sha256": digest.hexdigest(), "size": info.st_size, "executable": bool(info.st_mode & 0o111)})
        else:
            raise EvaluationError(f"cannot hash a special file: {path}")
    encoded = json.dumps(records, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def _apply_patch(project: Path, patch: Path) -> None:
    if patch.stat().st_size == 0:
        return
    before = hash_tree(project)
    with patch.open("rb") as source:
        check = subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "apply", "--check", "--whitespace=nowarn", "-"], cwd=project, env=_git_env(), stdin=source, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60, check=False)
    if check.returncode:
        raise EvaluationError(f"patch does not apply: {check.stderr.decode(errors='replace')[-2000:]}")
    with patch.open("rb") as source:
        applied = subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "apply", "--whitespace=nowarn", "-"], cwd=project, env=_git_env(), stdin=source, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60, check=False)
    if applied.returncode:
        raise EvaluationError(f"patch application failed: {applied.stderr.decode(errors='replace')[-2000:]}")
    if hash_tree(project) == before:
        raise EvaluationError(f"patch made no changes: {patch}")


def _apply_case_patch(project: Path, patch: Path, case: Mapping[str, Any], *, allow_reference_tests: bool = False) -> None:
    before = edit_snapshot.snapshot_tree(project)
    before_entries = {entry["path"]: entry for entry in before["entries"]}
    _apply_patch(project, patch)
    after = edit_snapshot.snapshot_tree(project)
    after_entries = {entry["path"]: entry for entry in after["entries"]}
    changes = _snapshot_changes(before, after)
    allowed_reference_changes = set()
    if allow_reference_tests:
        for reference_path in case.get("reference_test_files", []):
            if before_entries.get(reference_path) is not None:
                raise EvaluationError(f"reference test file must be absent from seed: {reference_path}")
            reference_entry = after_entries.get(reference_path)
            if reference_entry is None or reference_entry["kind"] != "file":
                raise EvaluationError(f"reference patch must add a regular test file: {reference_path}")
            allowed_reference_changes.add(reference_path)
            parts = PurePosixPath(reference_path).parts
            for index in range(1, len(parts)):
                parent = PurePosixPath(*parts[:index]).as_posix()
                parent_entry = after_entries.get(parent)
                if parent_entry is None or parent_entry["kind"] != "directory":
                    raise EvaluationError(f"reference test parent must be a real directory: {parent}")
                if before_entries.get(parent) is None:
                    allowed_reference_changes.add(parent)
    protected_changes = {
        path
        for path in changes
        if any(path == protected or path.startswith(protected + "/") or protected.startswith(path + "/") for protected in case["protected_paths"])
    }
    if protected_changes:
        raise EvaluationError(f"case patch changes protected paths: {', '.join(sorted(protected_changes))}")
    unexpected = changes - set(case["production_files"]) - allowed_reference_changes
    if unexpected:
        raise EvaluationError(f"case patch changes undeclared paths: {', '.join(sorted(unexpected))}")


def _initialize_empty_git(project: Path) -> None:
    root = Path(project).resolve(strict=True)
    if (root / ".git").exists() or (root / ".git").is_symlink():
        raise EvaluationError("trial Git metadata already exists")
    _git(root, ["init", "--quiet", "--initial-branch=main"])


def initialize_trial_git(project: str | Path) -> None:
    """Create a single seed-only commit with hooks, remotes, and user config disabled."""
    root = Path(project).resolve(strict=True)
    if not (root / ".git").exists():
        _git(root, ["init", "--quiet", "--initial-branch=main"])
    remotes = _git(root, ["remote"]).stdout.strip()
    commits_before = _git(root, ["rev-list", "--all", "--count"]).stdout.strip()
    if remotes or commits_before != "0":
        raise EvaluationError("trial Git metadata must be empty and have no remotes before seeding")
    _git(root, ["add", "--all", "--force"])
    _git(root, ["commit", "--quiet", "--no-gpg-sign", "-m", "evaluation seed"])
    remotes = _git(root, ["remote"]).stdout.strip()
    commits = _git(root, ["rev-list", "--all", "--count"]).stdout.strip()
    if remotes or commits != "1":
        raise EvaluationError("trial Git history must contain one local seed commit and no remotes")


def _copy_tree(source: Path, destination: Path) -> None:
    if destination.exists() or destination.is_symlink():
        raise EvaluationError(f"destination already exists: {destination}")
    shutil.copytree(source, destination, symlinks=True, copy_function=shutil.copy2)


def _load_case_sources(case_dir: Path, case: Mapping[str, Any]) -> dict[str, bytes]:
    files = {}
    for item in case["hidden_files"]:
        files[item["path"]] = _case_file(case_dir, item["source"], "hidden test source").read_bytes()
    return files


def _install_hidden(project: Path, hidden: Mapping[str, bytes]) -> None:
    for relative, content in hidden.items():
        path = project
        parts = PurePosixPath(relative).parts
        for part in parts[:-1]:
            path = path / part
            if path.is_symlink():
                raise EvaluationError(f"hidden test path crosses a symlink: {relative}")
            if path.exists() and not path.is_dir():
                raise EvaluationError(f"hidden test parent is not a directory: {relative}")
            path.mkdir(exist_ok=True)
        path = path / parts[-1]
        if path.exists() or path.is_symlink():
            raise EvaluationError(f"hidden test destination already exists in graded source: {relative}")
        path.write_bytes(content)


def _verify_hidden_absent(project: Path, case: Mapping[str, Any]) -> None:
    for item in case["hidden_files"]:
        path = project
        parts = PurePosixPath(item["path"]).parts
        for part in parts[:-1]:
            path = path / part
            if path.is_symlink():
                raise EvaluationError(f"reserved hidden path crosses a symlink: {item['path']}")
        path = path / parts[-1]
        if path.exists() or path.is_symlink():
            raise EvaluationError(f"reserved hidden path is present in the trial seed: {item['path']}")


def _existing_protection(project: Path, case: Mapping[str, Any]) -> list[str]:
    values = []
    for relative in case["protected_paths"]:
        path = project
        safe = True
        for part in PurePosixPath(relative).parts:
            path = path / part
            if path.is_symlink():
                safe = False
                break
        if not safe or not path.exists():
            continue
        values.append(relative)
    return values


def _mount_inputs(case: Mapping[str, Any], dependencies: Mapping[str, str | Path] | None, tools: Mapping[str, str | Path] | None) -> tuple[dict[str, Path], dict[str, Path]]:
    dependency_inputs = dependencies or {}
    tool_inputs = tools or {}
    unexpected_deps = set(dependency_inputs) - set(case.get("dependency_inputs", []))
    unexpected_tools = set(tool_inputs) - set(case.get("tool_inputs", []))
    if unexpected_deps:
        raise EvaluationError(f"undeclared dependency inputs: {', '.join(sorted(unexpected_deps))}")
    if unexpected_tools:
        raise EvaluationError(f"undeclared tool inputs: {', '.join(sorted(unexpected_tools))}")
    for name, value in list(dependency_inputs.items()) + list(tool_inputs.items()):
        if Path(value).is_symlink():
            raise EvaluationError(f"sandbox input cannot be a symbolic link: {name}")
    dep_map = {key: Path(value).resolve(strict=True) for key, value in dependency_inputs.items()}
    tool_map = {key: Path(value).resolve(strict=True) for key, value in tool_inputs.items()}
    missing_deps = set(case.get("dependency_inputs", [])) - dep_map.keys()
    missing_tools = set(case.get("tool_inputs", [])) - tool_map.keys()
    if missing_deps:
        raise EvaluationError(f"missing dependency inputs: {', '.join(sorted(missing_deps))}")
    if missing_tools:
        raise EvaluationError(f"missing tool inputs: {', '.join(sorted(missing_tools))}")
    return dep_map, tool_map


def _command_argv(command: Mapping[str, Any]) -> list[str]:
    argv = list(command["argv"])
    cwd = command["cwd"]
    if cwd != ".":
        argv = ["/usr/bin/env", "-C", f"{edit_sandbox.CONTAINER_PROJECT}/{cwd}", *argv]
    return argv


def _read_output(path: str | Path, maximum: int = 8 * 1024 * 1024) -> str:
    try:
        file = Path(path)
        size = file.stat().st_size
        with file.open("rb") as stream:
            if size <= maximum:
                raw = stream.read(maximum)
            else:
                half = maximum // 2
                head = stream.read(half)
                stream.seek(-half, os.SEEK_END)
                raw = head + b"\n... output elided ...\n" + stream.read(half)
    except OSError:
        return ""
    if len(raw) > maximum:
        raw = raw[:maximum]
    return raw.decode("utf-8", errors="replace")


def _strip_ansi(output: str) -> str:
    return ANSI_ESCAPE.sub("", output)


def _expected_test_execution(command: Mapping[str, Any], output: str) -> bool:
    lower = _strip_ansi(output).lower()
    argv = command["argv"]
    joined = " ".join(argv).lower()
    if "unittest" in joined or "pytest" in joined:
        match = re.search(r"ran\s+([1-9][0-9]*)\s+tests?", lower)
        return bool(match) or bool(re.search(r"[1-9][0-9]*\s+passed", lower))
    if re.search(r"\bgo\s+test\b", joined):
        return "\npass\n" in f"\n{lower}\n" or bool(re.search(r"^ok\s+", lower, re.MULTILINE)) or "--- pass:" in lower or "--- fail:" in lower
    if "cargo" in joined and "test" in joined:
        return bool(re.search(r"test result:.*\b[1-9][0-9]*\s+passed\b", lower)) or bool(re.search(r"test\s+[^\n]+\.\.\.\s+(ok|failed)", lower))
    if "vitest" in joined or "jest" in joined:
        return bool(re.search(r"test files?\s+.*\b[1-9][0-9]*\s+passed", lower)) or bool(re.search(r"tests?\s+.*\b[1-9][0-9]*\s+passed", lower)) or "test suite failed" in lower
    if "mvn" in joined or "maven" in joined:
        return bool(re.search(r"tests? run:\s*[1-9][0-9]*", lower))
    if "dotnet" in joined and "test" in joined:
        return bool(re.search(r"passed:\s*[1-9][0-9]*", lower)) or bool(re.search(r"failed:\s*[1-9][0-9]*", lower)) or bool(re.search(r"total tests:\s*[1-9][0-9]*", lower)) or bool(re.search(r"\bpassed\s+[^\s]+", lower)) or "passed!" in lower
    if "test" in joined or "check" in joined:
        return bool(re.search(r"\b[1-9][0-9]*\s+(passed|tests? passed|tests? run)\b", lower))
    return True


def _run_command(
    command: Mapping[str, Any],
    project: Path,
    state: Path,
    output: Path,
    mode: str,
    dependencies: Mapping[str, Path],
    tools: Mapping[str, Path],
    *,
    protect: Sequence[str] = (),
    verify_test_execution: bool = True,
    network: bool = False,
    private_profile: str | Path | None = None,
    auth_file: str | Path | None = None,
) -> dict[str, Any]:
    result = edit_sandbox.run(
        project,
        state,
        output,
        command["timeout_seconds"],
        mode,
        _command_argv(command),
        dependencies=dependencies,
        tools=tools,
        protect=_existing_protection(project, {"protected_paths": protect}),
        network=network,
        private_profile=private_profile,
        auth_file=auth_file,
    )
    stdout = _read_output(result["stdout_log"])
    stderr = _read_output(result["stderr_log"])
    combined = _strip_ansi(stdout + "\n" + stderr)
    markers = command.get("expected_output", [])
    markers_missing = [marker for marker in markers if marker not in combined]
    test_execution_observed = _expected_test_execution(command, combined) if verify_test_execution else None
    return {
        "command_id": command["id"],
        "exit_status": result["exit_status"],
        "timed_out": result["timed_out"],
        "elapsed_seconds": result["elapsed_seconds"],
        "process_cleanup": result["process_cleanup"],
        "stdout_log": result["stdout_log"],
        "stderr_log": result["stderr_log"],
        "test_execution_observed": test_execution_observed,
        "expected_output_missing": markers_missing,
        "passed": result["exit_status"] == 0 and not result["timed_out"] and not markers_missing and (not verify_test_execution or test_execution_observed),
    }


def _run_case_commands(
    commands: Sequence[Mapping[str, Any]],
    project: Path,
    work: Path,
    evidence: Path,
    role: str,
    dependencies: Mapping[str, Path],
    tools: Mapping[str, Path],
    case: Mapping[str, Any],
    *,
    mode: str = "grade",
    network: bool = False,
    state_group: str | None = None,
) -> list[dict[str, Any]]:
    results = []
    state = work / f"state-{state_group or role.split('-', 1)[0]}"
    for index, command in enumerate(commands):
        run_id = f"{role}-{index:02d}-{command['id']}"
        result = _run_command(
            command,
            project,
            state,
            evidence / run_id,
            mode,
            dependencies,
            tools,
            protect=case["protected_paths"],
            network=network,
        )
        results.append(result)
    return results


def _commands_pass(results: Sequence[Mapping[str, Any]]) -> bool:
    return bool(results) and all(item.get("passed") is True for item in results)


def _source_pin(case: Mapping[str, Any], corpus_path: Path) -> str:
    corpus = _read_json(corpus_path)
    matches = [repo for repo in corpus.get("repositories", []) if repo.get("id") == case["repo_id"]]
    if len(matches) != 1 or matches[0].get("commit") != case.get("source_pin"):
        raise EvaluationError(f"case pin does not match corpus for {case['repo_id']}")
    return matches[0]["commit"]


def _prepared_dependency_paths(case: Mapping[str, Any], state: Path, supplied: Mapping[str, Path]) -> dict[str, Path]:
    paths = dict(supplied)
    declared_outputs = case.get("dependency_output_paths", {})
    if not isinstance(declared_outputs, dict):
        raise EvaluationError("dependency_output_paths must be an object")
    for name in case.get("dependency_inputs", []):
        if name in paths:
            continue
        relative = _relative(declared_outputs.get(name, name), f"dependency output path for {name}")
        output = state
        safe = True
        parts = PurePosixPath(relative).parts
        for part in parts:
            output = output / part
            if output.is_symlink():
                safe = False
                break
        if safe and output.is_dir():
            paths[name] = output
    missing = set(case.get("dependency_inputs", [])) - paths.keys()
    if missing:
        raise EvaluationError(f"preparation did not produce declared dependencies: {', '.join(sorted(missing))}")
    return paths


def prepare_case_readiness(
    case_dir: str | Path,
    source_root: str | Path,
    output_root: str | Path,
    *,
    tools: Mapping[str, str | Path] | None = None,
    dependencies: Mapping[str, str | Path] | None = None,
    corpus_path: str | Path = DEFAULT_CORPUS,
) -> dict[str, Any]:
    """Run baseline, intentional seed failure, and reference acceptance in isolation."""
    case_path = Path(case_dir).resolve(strict=True)
    case = validate_case(case_path, corpus_path=corpus_path)
    pin = _source_pin(case, Path(corpus_path).resolve(strict=True))
    output = Path(output_root).resolve(strict=False)
    if output.exists() or output.is_symlink():
        raise EvaluationError(f"readiness output already exists: {output}")
    output.mkdir(parents=True)
    work, evidence = output / "work", output / "evidence"
    work.mkdir()
    evidence.mkdir()
    result: dict[str, Any] = {
        "schema": SCHEMA,
        "case_id": case["id"],
        "repo_id": case["repo_id"],
        "source_pin": pin,
        "case_sha256": hash_tree(case_path),
        "status": "not_ready",
        "behavioral_pass": False,
        "source_review": "pending",
        "checks": {},
        "created_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    try:
        supplied_tools = tools or {}
        supplied_dependencies = dependencies or {}
        if set(supplied_tools) - set(case.get("tool_inputs", [])):
            raise EvaluationError("readiness received undeclared tool inputs")
        if set(supplied_dependencies) - set(case.get("dependency_inputs", [])):
            raise EvaluationError("readiness received undeclared dependency inputs")
        if any(Path(value).is_symlink() for value in [*supplied_tools.values(), *supplied_dependencies.values()]):
            raise EvaluationError("readiness sandbox inputs cannot be symbolic links")
        input_tools = {name: Path(value).resolve(strict=True) for name, value in supplied_tools.items()}
        input_dependencies = {name: Path(value).resolve(strict=True) for name, value in supplied_dependencies.items()}
        missing_tools = set(case.get("tool_inputs", [])) - input_tools.keys()
        if missing_tools:
            raise EvaluationError(f"missing tool inputs: {', '.join(sorted(missing_tools))}")

        setup_project = work / "setup-project"
        export = export_source(source_root, pin, setup_project)
        _initialize_empty_git(setup_project)
        initialize_trial_git(setup_project)
        prep_state = work / "state-prepare"
        prep_state.mkdir()
        required_dependencies = set(case.get("dependency_inputs", []))
        reused_dependencies = required_dependencies <= input_dependencies.keys() and (bool(required_dependencies) or not case.get("setup_commands"))
        prep_records = []
        if not reused_dependencies:
            for index, command in enumerate(case.get("setup_commands", [])):
                prep_records.append(_run_command(command, setup_project, prep_state, evidence / f"prepare-{index:02d}-{command['id']}", "prepare", input_dependencies, input_tools, protect=case["protected_paths"], verify_test_execution=False, network=True))
        result["checks"]["preparation"] = {"passed": _commands_pass(prep_records) if prep_records else True, "mode": "reused-inputs" if reused_dependencies else "network-preparation", "commands": prep_records}
        if not result["checks"]["preparation"]["passed"]:
            raise EvaluationError("dependency preparation failed")
        prepared_dependencies = _prepared_dependency_paths(case, prep_state, input_dependencies)
        result["dependencies"] = {name: hash_tree(path) for name, path in sorted(prepared_dependencies.items())}
        result["source_tree_sha256"] = export["tree_sha256"]

        baseline_project = work / "baseline-project"
        export_source(source_root, pin, baseline_project)
        initialize_trial_git(baseline_project)
        baseline_runtime = _prepared_runtime_state(baseline_project, work, evidence, case, prepared_dependencies, input_tools, "baseline")
        result["checks"]["baseline_runtime_setup"] = {"passed": _commands_pass(baseline_runtime) if baseline_runtime else True, "commands": baseline_runtime}
        if not result["checks"]["baseline_runtime_setup"]["passed"]:
            raise EvaluationError("baseline runtime setup failed")
        baseline = _run_case_commands(case["public_commands"], baseline_project, work, evidence, "baseline-public", prepared_dependencies, input_tools, case)
        result["checks"]["baseline_public"] = {"passed": _commands_pass(baseline), "commands": baseline}
        if not result["checks"]["baseline_public"]["passed"]:
            raise EvaluationError("pinned upstream public checks did not pass")

        seed_project = work / "seed-project"
        export_source(source_root, pin, seed_project)
        _initialize_empty_git(seed_project)
        _apply_case_patch(seed_project, _case_file(case_path, case["seed_patch"], "seed_patch"), case)
        initialize_trial_git(seed_project)
        seed_snapshot = edit_snapshot.snapshot_tree(seed_project, excluded=case["generated_roots"])
        _verify_hidden_absent(seed_project, case)
        result["seed_project_path"] = "work/seed-project"
        seed_check_project = work / "seed-check-project"
        _copy_tree(seed_project, seed_check_project)
        _install_hidden(seed_check_project, _load_case_sources(case_path, case))
        seed_runtime = _prepared_runtime_state(seed_check_project, work, evidence, case, prepared_dependencies, input_tools, "seed")
        result["checks"]["seed_runtime_setup"] = {"passed": _commands_pass(seed_runtime) if seed_runtime else True, "commands": seed_runtime}
        if not result["checks"]["seed_runtime_setup"]["passed"]:
            raise EvaluationError("seed runtime setup failed")
        seed_hidden = _run_case_commands(
            [item for item in case["hidden_commands"] if item["id"] == case["expected_seed_failure"]["command_id"]],
            seed_check_project,
            work,
            evidence,
            "seed-hidden",
            prepared_dependencies,
            input_tools,
            case,
        )
        failure_record = seed_hidden[0]
        stdout = _read_output(failure_record["stdout_log"])
        stderr = _read_output(failure_record["stderr_log"])
        failure_observed = failure_record["exit_status"] in case["expected_seed_failure"]["exit_codes"] and not failure_record["timed_out"] and case["expected_seed_failure"]["diagnostic_contains"] in stdout + stderr
        result["checks"]["seed_hidden_failure"] = {"passed": failure_observed, "commands": seed_hidden}
        if not failure_observed:
            raise EvaluationError("seeded input did not fail the hidden contract for the expected reason")

        reference_project = work / "reference-project"
        _copy_tree(seed_project, reference_project)
        _apply_case_patch(reference_project, _case_file(case_path, case["reference_patch"], "reference_patch"), case, allow_reference_tests=True)
        reference_snapshot = edit_snapshot.snapshot_tree(reference_project, excluded=case["generated_roots"])
        reference_changes = _snapshot_changes(seed_snapshot, reference_snapshot)
        touched = sorted(reference_changes.intersection(case["production_files"]))
        if len(touched) < 2:
            raise EvaluationError("reference patch must change at least two declared production files")
        result["reference_production_files_changed"] = touched
        reference_bundle = evidence / "reference-bundle"
        edit_snapshot.capture_bundle(
            seed_snapshot,
            reference_project,
            reference_bundle,
            protected=case["protected_paths"],
            excluded=case["generated_roots"],
        )
        result["reference_bundle_path"] = "evidence/reference-bundle"
        reference_public_runtime = _prepared_runtime_state(reference_project, work, evidence, case, prepared_dependencies, input_tools, "reference-public")
        result["checks"]["reference_public_runtime_setup"] = {"passed": _commands_pass(reference_public_runtime) if reference_public_runtime else True, "commands": reference_public_runtime}
        if not result["checks"]["reference_public_runtime_setup"]["passed"]:
            raise EvaluationError("reference public runtime setup failed")
        reference_public = _run_case_commands(case["public_commands"], reference_project, work, evidence, "reference-public", prepared_dependencies, input_tools, case, state_group="reference-public")
        result["checks"]["reference_public"] = {"passed": _commands_pass(reference_public), "commands": reference_public}
        reference_hidden_project = work / "reference-hidden-project"
        _copy_tree(seed_project, reference_hidden_project)
        _apply_case_patch(reference_hidden_project, _case_file(case_path, case["reference_patch"], "reference_patch"), case, allow_reference_tests=True)
        _install_hidden(reference_hidden_project, _load_case_sources(case_path, case))
        reference_hidden_runtime = _prepared_runtime_state(reference_hidden_project, work, evidence, case, prepared_dependencies, input_tools, "reference-hidden")
        result["checks"]["reference_hidden_runtime_setup"] = {"passed": _commands_pass(reference_hidden_runtime) if reference_hidden_runtime else True, "commands": reference_hidden_runtime}
        if not result["checks"]["reference_hidden_runtime_setup"]["passed"]:
            raise EvaluationError("reference hidden runtime setup failed")
        reference_hidden = _run_case_commands(case["hidden_commands"], reference_hidden_project, work, evidence, "reference-hidden", prepared_dependencies, input_tools, case, state_group="reference-hidden")
        result["checks"]["reference_hidden"] = {"passed": _commands_pass(reference_hidden), "commands": reference_hidden}
        if not result["checks"]["reference_public"]["passed"] or not result["checks"]["reference_hidden"]["passed"]:
            raise EvaluationError("reference patch did not pass public and hidden checks")
        result["status"] = "ready"
        result["behavioral_pass"] = True
        result["seed_digest"] = seed_snapshot["digest"]
        result["reference_digest"] = reference_snapshot["digest"]
        result["readiness_error"] = None
    except Exception as exc:
        result["readiness_error"] = f"{type(exc).__name__}: {exc}"
    _write_json(output / "readiness.json", result)
    return result


def _snapshot_changes(before: Mapping[str, Any], after: Mapping[str, Any]) -> set[str]:
    left = {entry["path"]: entry for entry in before["entries"]}
    right = {entry["path"]: entry for entry in after["entries"]}
    return {path for path in left.keys() | right.keys() if left.get(path) != right.get(path)}


def _prepared_runtime_state(project: Path, work: Path, evidence: Path, case: Mapping[str, Any], dependencies: Mapping[str, Path], tools: Mapping[str, Path], role: str) -> list[dict[str, Any]]:
    records = []
    for index, command in enumerate(case.get("runtime_setup_commands", [])):
        records.append(_run_command(command, project, work / f"state-{role}", evidence / f"runtime-{role}-{index:02d}-{command['id']}", "grade", dependencies, tools, protect=case["protected_paths"], verify_test_execution=False))
        if not records[-1]["passed"]:
            break
    return records


def run_fake_agent(
    case_dir: str | Path,
    source_root: str | Path,
    attempt_dir: str | Path,
    argv: Sequence[str],
    *,
    tools: Mapping[str, str | Path] | None = None,
    dependencies: Mapping[str, str | Path] | None = None,
    timeout: float = 60,
    corpus_path: str | Path = DEFAULT_CORPUS,
) -> dict[str, Any]:
    """Run a local no-network agent command and capture its complete attempted tree diff."""
    case_path = Path(case_dir).resolve(strict=True)
    case = validate_case(case_path, corpus_path=corpus_path)
    source = Path(source_root).resolve(strict=True)
    attempt = Path(attempt_dir).resolve(strict=False)
    if attempt.exists() or attempt.is_symlink():
        raise EvaluationError(f"attempt output already exists: {attempt}")
    attempt.mkdir(parents=True)
    evidence = attempt / "evidence"
    evidence.mkdir()
    state = attempt / "state-agent"
    deps, tool_map = _mount_inputs(case, dependencies, tools)
    seed_project = attempt / "seed-project"
    export_source(source, case["source_pin"], seed_project)
    _initialize_empty_git(seed_project)
    _apply_case_patch(seed_project, _case_file(case_path, case["seed_patch"], "seed_patch"), case)
    initialize_trial_git(seed_project)
    project = attempt / "agent-project"
    _copy_tree(seed_project, project)
    seed_snapshot = edit_snapshot.snapshot_tree(seed_project, excluded=case["generated_roots"])
    _verify_hidden_absent(seed_project, case)
    runtime = _prepared_runtime_state(project, attempt, evidence, case, deps, tool_map, "agent")
    if not _commands_pass(runtime) and runtime:
        result = {"schema": SCHEMA, "case_id": case["id"], "completed": False, "patch_valid": False, "protection_status": "unavailable", "behavioral_pass": None, "source_review": "pending", "acceptance_status": "awaiting_grade", "success": False, "runtime_setup": runtime}
        _write_json(attempt / "attempt.json", result)
        return result
    command = {"id": "fake-agent", "argv": list(argv), "cwd": ".", "timeout_seconds": timeout, "expected_output": []}
    execution = _run_command(command, project, state, evidence / "agent", "grade", deps, tool_map, protect=case["protected_paths"], verify_test_execution=False)
    bundle_path = attempt / "bundle"
    bundle = edit_snapshot.capture_bundle(seed_snapshot, project, bundle_path, protected=case["protected_paths"], excluded=case["generated_roots"])
    valid_patch = not bundle["violations"]
    stderr = _read_output(execution["stderr_log"]).lower()
    protection_denied = any(fragment in stderr for fragment in ("read-only file system", "permission denied", "operation not permitted"))
    result = {
        "schema": SCHEMA,
        "case_id": case["id"],
        "execution": execution,
        "completed": execution["exit_status"] == 0 and not execution["timed_out"],
        "patch_valid": valid_patch,
        "behavioral_pass": None,
        "source_review": "pending",
        "acceptance_status": "awaiting_grade",
        "protection_status": "violation" if any(item["code"] == "protected_path" for item in bundle["violations"]) else "unchanged",
        "protection_denied": protection_denied,
        "bundle_path": str(bundle_path),
        "seed_project": str(seed_project),
        "agent_project": str(project),
        "bundle_sha256": hashlib.sha256((bundle_path / "bundle.json").read_bytes()).hexdigest(),
        "change_count": len(bundle["changes"]),
        "violations": bundle["violations"],
        "success": False,
    }
    _write_json(attempt / "attempt.json", result)
    return result


def replay_and_grade(
    case_dir: str | Path,
    seed_project: str | Path,
    bundle_dir: str | Path,
    grade_dir: str | Path,
    *,
    tools: Mapping[str, str | Path] | None = None,
    dependencies: Mapping[str, str | Path] | None = None,
    trial_completed: bool = True,
    corpus_path: str | Path = DEFAULT_CORPUS,
) -> dict[str, Any]:
    """Replay a captured patch into a fresh tree, add hidden tests, then run trusted checks."""
    case_path = Path(case_dir).resolve(strict=True)
    case = validate_case(case_path, corpus_path=corpus_path)
    seed = Path(seed_project).resolve(strict=True)
    bundle = Path(bundle_dir).resolve(strict=True)
    grade = Path(grade_dir).resolve(strict=False)
    if grade.exists() or grade.is_symlink():
        raise EvaluationError(f"grade output already exists: {grade}")
    grade.mkdir(parents=True)
    work, evidence = grade / "work", grade / "evidence"
    work.mkdir()
    evidence.mkdir()
    deps, tool_map = _mount_inputs(case, dependencies, tools)
    public_project = work / "public-project"
    hidden_project = work / "hidden-project"
    output: dict[str, Any] = {
        "schema": SCHEMA,
        "case_id": case["id"],
        "trial_completed": trial_completed,
        "replay_valid": False,
        "patch_valid": False,
        "protection_status": "unknown",
        "behavioral_pass": False,
        "source_review": "pending",
        "acceptance_status": "failed",
        "checks": {},
        "success": False,
    }
    try:
        public_snapshot = edit_snapshot.apply_bundle(seed, bundle, public_project, protected=case["protected_paths"], excluded=case["generated_roots"])
        hidden_snapshot = edit_snapshot.apply_bundle(seed, bundle, hidden_project, protected=case["protected_paths"], excluded=case["generated_roots"])
        if public_snapshot != hidden_snapshot:
            raise EvaluationError("public and hidden replays do not have the same patch identity")
        output["replay_valid"] = True
        output["patch_valid"] = True
        output["replay_digest"] = public_snapshot["digest"]
        output["protection_status"] = "unchanged"
    except Exception as exc:
        output["replay_error"] = f"{type(exc).__name__}: {exc}"
        output["protection_status"] = "violation" if "protected" in str(exc).lower() else "unavailable"
        _write_json(grade / "grade.json", output)
        return output

    try:
        _install_hidden(hidden_project, _load_case_sources(case_path, case))
    except Exception as exc:
        output["replay_valid"] = False
        output["patch_valid"] = False
        output["replay_error"] = f"{type(exc).__name__}: {exc}"
        output["protection_status"] = "violation"
        _write_json(grade / "grade.json", output)
        return output

    public_runtime = _prepared_runtime_state(public_project, work, evidence, case, deps, tool_map, "grade-public")
    hidden_runtime = _prepared_runtime_state(hidden_project, work, evidence, case, deps, tool_map, "grade-hidden")
    output["checks"]["public_runtime_setup"] = {"passed": _commands_pass(public_runtime) if public_runtime else True, "commands": public_runtime}
    output["checks"]["hidden_runtime_setup"] = {"passed": _commands_pass(hidden_runtime) if hidden_runtime else True, "commands": hidden_runtime}
    public = _run_case_commands(case["public_commands"], public_project, work, evidence, "grade-public", deps, tool_map, case, state_group="grade-public") if output["checks"]["public_runtime_setup"]["passed"] else []
    hidden_results = _run_case_commands(case["hidden_commands"], hidden_project, work, evidence, "grade-hidden", deps, tool_map, case, state_group="grade-hidden") if output["checks"]["hidden_runtime_setup"]["passed"] else []
    output["checks"]["public"] = {"passed": _commands_pass(public) if public else False, "commands": public}
    output["checks"]["hidden"] = {"passed": _commands_pass(hidden_results) if hidden_results else False, "commands": hidden_results}
    output["behavioral_pass"] = bool(output["replay_valid"] and output["checks"]["public_runtime_setup"]["passed"] and output["checks"]["hidden_runtime_setup"]["passed"] and output["checks"]["public"]["passed"] and output["checks"]["hidden"]["passed"])
    output["success"] = False
    if not trial_completed:
        output["acceptance_status"] = "incomplete"
    elif output["behavioral_pass"]:
        output["acceptance_status"] = "pending_source_review"
    else:
        output["acceptance_status"] = "failed"
    _write_json(grade / "grade.json", output)
    return output


def validate_manifest(
    manifest_path: str | Path,
    *,
    corpus_path: str | Path = DEFAULT_CORPUS,
    artifact_paths: Mapping[str, str | Path] | None = None,
) -> dict[str, Any]:
    """Verify a frozen 24-cell campaign and every bound local artifact digest."""
    path = Path(manifest_path).resolve(strict=True)
    manifest = _read_json(path)
    if manifest.get("schema") != 1 or manifest.get("status") != "ready":
        raise EvaluationError("campaign manifest must use schema 1 and status ready")
    corpus = _read_json(Path(corpus_path).resolve(strict=True))
    expected_repos = corpus.get("repositories", [])
    if len(expected_repos) != 6:
        raise EvaluationError("completed-edit campaign requires exactly six pinned repositories")
    expected_by_id = {repo["id"]: repo["commit"] for repo in expected_repos if isinstance(repo, dict)}
    if len(expected_by_id) != 6:
        raise EvaluationError("corpus repositories must have six unique pinned ids")
    cases = manifest.get("cases")
    if not isinstance(cases, list) or len(cases) != 6:
        raise EvaluationError("manifest must bind exactly six ready cases")
    case_ids = [item.get("id") for item in cases if isinstance(item, dict)]
    if case_ids != [repo["id"] for repo in expected_repos]:
        raise EvaluationError("manifest case ids must follow the six corpus pins exactly")
    for item in cases:
        case_id = item["id"]
        if item.get("repo_id") != case_id or item.get("pin") != expected_by_id[case_id]:
            raise EvaluationError(f"manifest source pin mismatch for {case_id}")
        for field in ("case_sha256", "readiness_sha256"):
            if not isinstance(item.get(field), str) or not SHA256.fullmatch(item[field]):
                raise EvaluationError(f"manifest {field} is invalid for {case_id}")
        for field in ("case_path", "readiness_path"):
            relative = _relative(item.get(field), f"{case_id}.{field}")
            artifact = _manifest_artifact(path.parent, relative, f"{case_id}.{field}")
            expected_hash = item["case_sha256"] if field == "case_path" else item["readiness_sha256"]
            actual_hash = hash_tree(artifact) if artifact.is_dir() else _hash_file(artifact)
            if actual_hash != expected_hash:
                raise EvaluationError(f"manifest {field} digest is stale for {case_id}")
        readiness = _read_json(_manifest_artifact(path.parent, item["readiness_path"], f"{case_id}.readiness_path"))
        if readiness.get("status") != "ready" or readiness.get("behavioral_pass") is not True or readiness.get("case_id") != case_id or readiness.get("source_pin") != item["pin"] or readiness.get("case_sha256") != item["case_sha256"] or readiness.get("source_review") != "passed":
            raise EvaluationError(f"readiness record is not successful and pin-bound for {case_id}")

    schedule = manifest.get("schedule")
    if not isinstance(schedule, list) or len(schedule) != 24:
        raise EvaluationError("manifest schedule must contain exactly 24 scored calls")
    expected_schedule = []
    for repeat, arms in ((1, ("native", "code-kb")), (2, ("code-kb", "native"))):
        for item in cases:
            expected_schedule.extend({"case_id": item["id"], "arm": arm, "repeat": repeat} for arm in arms)
    normalized = [{"case_id": item.get("case_id"), "arm": item.get("arm"), "repeat": item.get("repeat")} for item in schedule if isinstance(item, dict)]
    if normalized != expected_schedule:
        raise EvaluationError("manifest schedule must include each case in the frozen native/code-kb, code-kb/native pair order")
    controls = manifest.get("controls")
    required_controls = {"model", "effort", "timeout_seconds", "codex_sha256", "protocol_sha256", "corpus_sha256", "snapshot_sha256", "sandbox_sha256", "runner_sha256", "extractor_sha256", "binary_sha256", "guidance_sha256"}
    if not isinstance(controls, dict):
        raise EvaluationError("manifest controls must be an object")
    missing_controls = required_controls - controls.keys()
    if missing_controls:
        raise EvaluationError(f"manifest controls are missing: {', '.join(sorted(missing_controls))}")
    if controls.get("model") != "gpt-6.1-sol" or controls.get("effort") != "high" or controls.get("timeout_seconds") != 900:
        raise EvaluationError("manifest model, effort, and task timeout do not match the frozen protocol")
    artifact_map = manifest.get("artifacts")
    if not isinstance(artifact_map, dict):
        raise EvaluationError("manifest artifacts must be an object")
    overrides = dict(artifact_paths or {})
    control_names = {name.removesuffix("_sha256") for name in controls if name.endswith("_sha256")}
    if set(overrides) - control_names:
        raise EvaluationError(f"artifact overrides are undeclared: {', '.join(sorted(set(overrides) - control_names))}")
    missing_overrides = control_names - overrides.keys()
    if missing_overrides:
        raise EvaluationError(f"missing control artifact paths: {', '.join(sorted(missing_overrides))}")
    live_artifacts = {
        "runner": Path(__file__).resolve(strict=True),
        "snapshot": Path(edit_snapshot.__file__).resolve(strict=True),
        "sandbox": Path(edit_sandbox.__file__).resolve(strict=True),
        "protocol": (REPO_ROOT / "evals/edit-2026-10-27/protocol.md").resolve(strict=True),
        "corpus": Path(corpus_path).resolve(strict=True),
    }
    for key in sorted(name for name in controls if name.endswith("_sha256")):
        expected_hash = controls.get(key)
        if not isinstance(expected_hash, str) or not SHA256.fullmatch(expected_hash):
            raise EvaluationError(f"manifest control digest is invalid: {key}")
        artifact_name = key.removesuffix("_sha256")
        artifact_path = artifact_map.get(artifact_name)
        if not isinstance(artifact_path, str):
            raise EvaluationError(f"manifest does not bind a path for control artifact {artifact_name}")
        _relative(artifact_path, f"artifact path {artifact_name}")
        supplied_path = Path(overrides[artifact_name])
        if not supplied_path.is_absolute():
            raise EvaluationError(f"control artifact must be an absolute path: {artifact_name}")
        if supplied_path.is_symlink():
            raise EvaluationError(f"control artifact cannot be a symbolic link: {artifact_name}")
        if not supplied_path.is_file() and not supplied_path.is_dir():
            raise EvaluationError(f"control artifact must be a regular file or directory: {artifact_name}")
        artifact = supplied_path.resolve(strict=True)
        if artifact_name in live_artifacts and artifact != live_artifacts[artifact_name]:
            raise EvaluationError(f"control artifact must name the active source: {artifact_name}")
        digest = hash_tree(artifact) if artifact.is_dir() else _hash_file(artifact)
        if digest != expected_hash:
            raise EvaluationError(f"manifest control artifact digest is stale: {artifact_name}")
        if artifact_name == "corpus" and _hash_file(Path(corpus_path).resolve(strict=True)) != expected_hash:
            raise EvaluationError("manifest corpus digest does not match the evaluated corpus")
    return {"valid": True, "case_count": 6, "scored_calls": 24}


def _hash_file(path: Path) -> str:
    if path.is_symlink() or not path.is_file():
        raise EvaluationError(f"artifact must be a regular file: {path}")
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _manifest_artifact(root: Path, relative: str, label: str) -> Path:
    path = root
    for part in PurePosixPath(_relative(relative, label)).parts:
        path = path / part
        if path.is_symlink():
            raise EvaluationError(f"manifest artifact crosses a symlink: {label}")
    if not path.exists():
        raise EvaluationError(f"manifest artifact is missing: {label}")
    return path


def _record_is_accepted(record: Mapping[str, Any]) -> bool:
    completion_fields = [key for key in ("completed", "trial_completed") if key in record]
    validity_fields = [key for key in ("patch_valid", "replay_valid") if key in record]
    completed = bool(completion_fields) and all(record[key] is True for key in completion_fields)
    valid = bool(validity_fields) and all(record[key] is True for key in validity_fields)
    return bool(
        record.get("source_review") == "passed"
        and record.get("acceptance_status") == "accepted"
        and completed
        and valid
        and record.get("protection_status") == "unchanged"
        and record.get("behavioral_pass") is True
    )


def summarize_campaign(records: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    """Report completed attempts and outcomes without treating tokens as billing."""
    rows = []
    for record in records:
        trace_metrics = None
        trace_error = None
        trace_path = record.get("trace_path")
        if isinstance(trace_path, str):
            try:
                raw_path = Path(trace_path)
                if raw_path.is_symlink():
                    raise EvaluationError("trace cannot be a symbolic link")
                path = raw_path.resolve(strict=True)
                if not path.is_file() or path.stat().st_size > MAX_TRACE_BYTES:
                    raise EvaluationError("trace must be a bounded regular file")
                events = []
                with path.open("r", encoding="utf-8") as source:
                    for line in source:
                        if len(events) >= MAX_TRACE_EVENTS:
                            raise EvaluationError("trace event limit exceeded")
                        if len(line.encode("utf-8")) > 2 * 1024 * 1024:
                            raise EvaluationError("trace event exceeds the size limit")
                        events.append(json.loads(line))
                trace_metrics = comparison_metrics.summarize_trace(events)
            except (EvaluationError, OSError, UnicodeError, json.JSONDecodeError) as exc:
                trace_error = f"{type(exc).__name__}: {exc}"
        source_review = record.get("source_review", "pending")
        behavioral_pass = record.get("behavioral_pass")
        acceptance_status = record.get("acceptance_status")
        if acceptance_status is None:
            if behavioral_pass is True and source_review == "pending":
                acceptance_status = "pending_source_review"
            elif behavioral_pass is False:
                acceptance_status = "failed"
            else:
                acceptance_status = "awaiting_grade"
        accepted = _record_is_accepted({**record, "acceptance_status": acceptance_status})
        rows.append({
            "case_id": record.get("case_id"),
            "arm": record.get("arm"),
            "repeat": record.get("repeat"),
            "completed": record.get("completed") if "completed" in record else record.get("trial_completed"),
            "patch_valid": record.get("patch_valid"),
            "replay_valid": record.get("replay_valid"),
            "protection_status": record.get("protection_status"),
            "success": accepted,
            "behavioral_pass": behavioral_pass,
            "source_review": source_review,
            "acceptance_status": acceptance_status,
            "input_tokens": record.get("input_tokens") if record.get("input_tokens") is not None else trace_metrics.get("input_tokens") if trace_metrics else None,
            "output_tokens": record.get("output_tokens") if record.get("output_tokens") is not None else trace_metrics.get("output_tokens") if trace_metrics else None,
            "cache_tokens": record.get("cache_tokens") if record.get("cache_tokens") is not None else trace_metrics.get("cache_tokens") if trace_metrics else None,
            "task_seconds": record.get("task_seconds"),
            "billing": record.get("billing"),
            "tool_calls": trace_metrics.get("tool_calls") if trace_metrics else None,
            "code_kb_calls": trace_metrics.get("code_kb_calls") if trace_metrics else None,
            "trace_completed": trace_metrics.get("completed") if trace_metrics else None,
            "trace_errors": trace_metrics.get("errors") if trace_metrics else None,
            "trace_error": trace_error,
        })
    return {
        "attempt_count": len(rows),
        "success_count": sum(row["success"] is True for row in rows),
        "accepted_count": sum(row["success"] is True for row in rows),
        "completed_count": sum(row["completed"] is True for row in rows),
        "invalid_patch_count": sum(row["patch_valid"] is False for row in rows),
        "behavioral_pass_count": sum(row["behavioral_pass"] is True for row in rows),
        "pending_review_count": sum(row["acceptance_status"] == "pending_source_review" for row in rows),
        "records": rows,
    }


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    validate = commands.add_parser("validate-case")
    validate.add_argument("--case-dir", required=True)
    validate.add_argument("--corpus", default=str(DEFAULT_CORPUS))
    export = commands.add_parser("export-source")
    export.add_argument("--repository", required=True)
    export.add_argument("--commit", required=True)
    export.add_argument("--destination", required=True)
    readiness = commands.add_parser("readiness")
    readiness.add_argument("--case-dir", required=True)
    readiness.add_argument("--source-root", required=True)
    readiness.add_argument("--output", required=True)
    readiness.add_argument("--corpus", default=str(DEFAULT_CORPUS))
    readiness.add_argument("--tool", action="append", default=[])
    readiness.add_argument("--dependency", action="append", default=[])
    fake = commands.add_parser("fake-agent")
    fake.add_argument("--case-dir", required=True)
    fake.add_argument("--source-root", required=True)
    fake.add_argument("--attempt-dir", required=True)
    fake.add_argument("--corpus", default=str(DEFAULT_CORPUS))
    fake.add_argument("--tool", action="append", default=[])
    fake.add_argument("--dependency", action="append", default=[])
    fake.add_argument("--timeout", type=float, default=60)
    fake.add_argument("argv", nargs=argparse.REMAINDER)
    grade = commands.add_parser("grade-bundle")
    grade.add_argument("--case-dir", required=True)
    grade.add_argument("--seed-project", required=True)
    grade.add_argument("--bundle", required=True)
    grade.add_argument("--output", required=True)
    grade.add_argument("--corpus", default=str(DEFAULT_CORPUS))
    grade.add_argument("--tool", action="append", default=[])
    grade.add_argument("--dependency", action="append", default=[])
    grade.add_argument("--trial-completed", action=argparse.BooleanOptionalAction, default=True)
    manifest = commands.add_parser("validate-manifest")
    manifest.add_argument("--manifest", required=True)
    manifest.add_argument("--corpus", default=str(DEFAULT_CORPUS))
    manifest.add_argument("--artifact", action="append", default=[])
    summary = commands.add_parser("summarize")
    summary.add_argument("records", nargs="+", help="attempt.json or grade.json files")
    return parser


def _named_paths(values: Sequence[str], label: str) -> dict[str, str]:
    result = {}
    for value in values:
        if "=" not in value:
            raise EvaluationError(f"{label} must use NAME=ABSOLUTE_PATH: {value!r}")
        name, path = value.split("=", 1)
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,99}", name) or not Path(path).is_absolute():
            raise EvaluationError(f"invalid {label}: {value!r}")
        if name in result:
            raise EvaluationError(f"duplicate {label} name: {name}")
        result[name] = path
    return result


def main(arguments: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(arguments)
    try:
        if args.command == "validate-case":
            value = validate_case(args.case_dir, corpus_path=args.corpus)
        elif args.command == "export-source":
            value = export_source(args.repository, args.commit, args.destination)
        elif args.command == "readiness":
            value = prepare_case_readiness(args.case_dir, args.source_root, args.output, tools=_named_paths(args.tool, "tool"), dependencies=_named_paths(args.dependency, "dependency"), corpus_path=args.corpus)
        elif args.command == "fake-agent":
            argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
            value = run_fake_agent(args.case_dir, args.source_root, args.attempt_dir, argv, tools=_named_paths(args.tool, "tool"), dependencies=_named_paths(args.dependency, "dependency"), timeout=args.timeout, corpus_path=args.corpus)
        elif args.command == "grade-bundle":
            value = replay_and_grade(args.case_dir, args.seed_project, args.bundle, args.output, tools=_named_paths(args.tool, "tool"), dependencies=_named_paths(args.dependency, "dependency"), trial_completed=args.trial_completed, corpus_path=args.corpus)
        elif args.command == "validate-manifest":
            value = validate_manifest(args.manifest, corpus_path=args.corpus, artifact_paths=_named_paths(args.artifact, "artifact"))
        elif args.command == "summarize":
            value = summarize_campaign([_read_json(Path(item).resolve(strict=True)) for item in args.records])
        else:
            raise EvaluationError("unsupported command")
        print(json.dumps(value, sort_keys=True, indent=2))
        if args.command == "readiness" and value.get("status") != "ready":
            return 1
        if args.command == "fake-agent" and not value.get("completed"):
            return 1
        if args.command == "grade-bundle" and not value.get("behavioral_pass"):
            return 1
        return 0
    except (EvaluationError, edit_sandbox.SandboxError, edit_snapshot.SnapshotError, OSError, subprocess.SubprocessError, tarfile.TarError) as exc:
        print(json.dumps({"error": f"{type(exc).__name__}: {exc}"}, sort_keys=True), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
