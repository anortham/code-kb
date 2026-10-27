"""Dispatch frozen completed-edit trials through the reviewed local sandbox."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Mapping, Sequence
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts import comparison_campaign, comparison_metrics, edit_evaluation, edit_sandbox, edit_snapshot


SCHEMA = 1
MAX_SOURCE_AUDIT_BYTES = 256 * 1024 * 1024
SECRET_ENV_MARKERS = ("API_KEY", "ACCESS_KEY", "SECRET", "TOKEN", "PASSWORD", "PASSWD", "AUTHORIZATION", "PRIVATE_KEY")
TRACE_SECRET_FIELDS = ("apikey", "accesskey", "secret", "token", "password", "passwd", "credential", "authorization", "privatekey", "bearer")


class CampaignError(RuntimeError):
    pass


def _sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _path_digest(path: Path) -> str:
    return edit_evaluation.hash_tree(path) if path.is_dir() else _sha256_file(path)


def _read_json(path: str | Path, label: str) -> dict[str, Any]:
    target = Path(path).expanduser()
    if not target.is_absolute():
        raise CampaignError(f"{label} must be an absolute path")
    if target.is_symlink() or not target.is_file():
        raise CampaignError(f"{label} must be a regular file: {target}")
    try:
        value = json.loads(target.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise CampaignError(f"cannot read {label}: {error}") from error
    if not isinstance(value, dict):
        raise CampaignError(f"{label} must contain a JSON object")
    return value


def _atomic_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    if temporary.exists() or temporary.is_symlink():
        raise CampaignError(f"temporary record already exists: {temporary}")
    data = json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    descriptor = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise


def _resolve_path(value: str | Path, label: str, *, must_exist: bool = True) -> Path:
    path = Path(value).expanduser()
    if not path.is_absolute():
        raise CampaignError(f"{label} must be an absolute path: {path}")
    if path.is_symlink():
        raise CampaignError(f"{label} cannot be a symbolic link: {path}")
    try:
        return path.resolve(strict=must_exist)
    except OSError as error:
        raise CampaignError(f"{label} is unavailable: {path}") from error


def _artifact_map(values: Mapping[str, str | Path]) -> dict[str, Path]:
    if not isinstance(values, Mapping) or not values:
        raise CampaignError("artifact_paths must be a non-empty object")
    paths = {}
    for name, value in values.items():
        if not isinstance(name, str) or not name or not isinstance(value, (str, Path)):
            raise CampaignError("artifact_paths must map artifact names to absolute paths")
        paths[name] = _resolve_path(value, f"artifact {name}")
    return paths


def _contained_path(root: Path, relative: str, label: str) -> Path:
    candidate = PurePosixPath(relative)
    if candidate.is_absolute() or not candidate.parts or any(part in ("", ".", "..") for part in candidate.parts):
        raise CampaignError(f"{label} must be a safe relative path")
    root = root.resolve(strict=True)
    path = root.joinpath(*candidate.parts)
    if path.is_symlink():
        raise CampaignError(f"{label} cannot be a symbolic link: {path}")
    resolved = path.resolve(strict=True)
    if not resolved.is_relative_to(root):
        raise CampaignError(f"{label} escapes its study root")
    return resolved


def _timestamp() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def _has_code_kb_index(project: Path) -> bool:
    path = project / ".code-kb"
    return path.exists() or path.is_symlink()


def _git(root: Path, arguments: Sequence[str]) -> str:
    environment = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": os.environ.get("HOME", str(Path.home())),
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_TERMINAL_PROMPT": "0",
    }
    result = subprocess.run(["git", "-C", str(root), *arguments], capture_output=True, text=True, env=environment, check=False)
    if result.returncode != 0:
        raise CampaignError(f"Git identity check failed for {root}: {result.stderr.strip() or result.returncode}")
    return result.stdout.strip()


def _validate_auth_file(path: str | Path) -> Path:
    auth = _resolve_path(path, "saved Codex authentication")
    if not auth.is_file():
        raise CampaignError("saved Codex authentication must be a regular file")
    details = auth.stat()
    if details.st_mode & 0o077:
        raise CampaignError("saved Codex authentication must not be group- or world-accessible")
    if details.st_uid != os.getuid():
        raise CampaignError("saved Codex authentication must be owned by the current user")
    return auth


def _validate_runtime_bindings(value: dict[str, Any]) -> None:
    if value.get("schema") != SCHEMA:
        raise CampaignError("runtime bindings must use schema 1")
    if not isinstance(value.get("cases"), dict) or not isinstance(value.get("seeds"), dict):
        raise CampaignError("runtime bindings need cases and seeds objects")


def _verify_seed(case_item: Mapping[str, Any], case: Mapping[str, Any], bindings: Mapping[str, Any], study_root: Path) -> tuple[Path, Path, str]:
    case_id = case["id"]
    pin = case_item.get("pin")
    runtime_case = bindings.get("cases", {}).get(case_id)
    seed_binding = bindings.get("seeds", {}).get(case_id)
    if not isinstance(runtime_case, dict) or not isinstance(seed_binding, dict):
        raise CampaignError(f"runtime bindings are missing case inputs for {case_id}")
    if seed_binding.get("source_pin") != pin or case.get("source_pin") != pin:
        raise CampaignError(f"source pin does not match frozen case {case_id}")
    source_root = _resolve_path(runtime_case.get("source_root", ""), f"{case_id} pinned source")
    seed_project = _resolve_path(seed_binding.get("seed_project", ""), f"{case_id} durable seed")
    if not source_root.is_dir() or not seed_project.is_dir():
        raise CampaignError(f"source and seed bindings must be directories for {case_id}")
    if _has_code_kb_index(seed_project):
        raise CampaignError(f"durable seed contains a preexisting code-kb index for {case_id}")
    if _git(source_root, ["rev-parse", "HEAD"]) != pin:
        raise CampaignError(f"pinned source checkout changed for {case_id}")
    if _git(seed_project, ["remote"]) or _git(seed_project, ["rev-list", "--all", "--count"]) != "1":
        raise CampaignError(f"durable seed Git history is not a single local commit for {case_id}")
    seed_digest = seed_binding.get("seed_digest")
    if not isinstance(seed_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", seed_digest):
        raise CampaignError(f"durable seed digest is invalid for {case_id}")
    actual = edit_snapshot.snapshot_tree(seed_project, excluded=case["generated_roots"])
    if actual["digest"] != seed_digest:
        raise CampaignError(f"durable seed digest changed for {case_id}")
    readiness_path = _contained_path(study_root, case_item["readiness_path"], f"{case_id} readiness record")
    if _sha256_file(readiness_path) != case_item.get("readiness_sha256"):
        raise CampaignError(f"readiness record digest is stale for {case_id}")
    readiness = _read_json(readiness_path, f"{case_id} readiness record")
    if readiness.get("status") != "ready" or readiness.get("behavioral_pass") is not True or readiness.get("source_review") != "passed" or readiness.get("case_id") != case_id:
        raise CampaignError(f"readiness record is not accepted for {case_id}")
    if readiness.get("source_pin") != pin or readiness.get("seed_digest") != seed_digest:
        raise CampaignError(f"durable seed does not match ready manifest record for {case_id}")
    if readiness.get("case_sha256") != case_item.get("case_sha256"):
        raise CampaignError(f"case identity does not match ready manifest record for {case_id}")
    return source_root, seed_project, seed_digest


def _named_inputs(case: Mapping[str, Any], runtime_case: Mapping[str, Any]) -> tuple[dict[str, Path], dict[str, Path]]:
    dependencies = runtime_case.get("dependencies", {})
    tools = runtime_case.get("tools", {})
    if not isinstance(dependencies, dict) or not isinstance(tools, dict):
        raise CampaignError(f"runtime inputs must be named maps for {case['id']}")
    dependency_names = set(case.get("dependency_inputs", []))
    tool_names = set(case.get("tool_inputs", []))
    if set(dependencies) != dependency_names:
        raise CampaignError(f"dependency bindings do not match case contract for {case['id']}")
    if set(tools) != tool_names:
        raise CampaignError(f"tool bindings do not match case contract for {case['id']}")
    dependency_paths = {name: _resolve_path(path, f"dependency {name}") for name, path in dependencies.items()}
    tool_paths = {name: _resolve_path(path, f"tool {name}") for name, path in tools.items()}
    if any(not path.is_dir() and not path.is_file() for path in (*dependency_paths.values(), *tool_paths.values())):
        raise CampaignError(f"runtime bindings contain a non-file input for {case['id']}")
    if "codex" in tool_paths or "intelligence" in tool_paths:
        raise CampaignError(f"case tool names conflict with the agent runtime for {case['id']}")
    return dependency_paths, tool_paths


def _verify_named_input_artifacts(case_id: str, dependencies: Mapping[str, Path], tools: Mapping[str, Path], artifacts: Mapping[str, Path]) -> None:
    for kind, values in (("dependencies", dependencies), ("tools", tools)):
        for name, path in values.items():
            artifact_name = f"{kind}_{case_id}_{name}"
            frozen = artifacts.get(artifact_name)
            if frozen is None or (path != frozen and _path_digest(path) != _path_digest(frozen)):
                raise CampaignError(f"runtime input does not match its frozen artifact: {artifact_name}")


def _profile_files(profile_root: Path, arm: str) -> tuple[Path, list[tuple[str, bytes]]]:
    if arm not in ("native", "code-kb"):
        raise CampaignError(f"unsupported campaign arm: {arm}")
    profile = profile_root / arm
    if profile.is_symlink() or not profile.is_dir():
        raise CampaignError(f"frozen profile directory is unavailable: {arm}")
    files = []
    for path in sorted(profile.rglob("*")):
        if path.is_symlink():
            raise CampaignError(f"frozen profile contains a symbolic link: {path.relative_to(profile)}")
        if path.is_file():
            relative = path.relative_to(profile).as_posix()
            payload = path.read_bytes()
            files.append((relative, payload))
    if not files or not any(name == "config.toml" for name, _ in files):
        raise CampaignError(f"frozen profile is missing config.toml: {arm}")
    return profile, files


def _framed_payload(entries: Sequence[tuple[str, bytes]]) -> bytes:
    payload = bytearray()
    for name, content in entries:
        header = f"\nFILE {name} {len(content)}\n".encode("utf-8")
        payload.extend(header)
        payload.extend(content)
        payload.extend(b"\n")
        if len(payload) > MAX_SOURCE_AUDIT_BYTES:
            raise CampaignError(f"outgoing text bundle exceeds {MAX_SOURCE_AUDIT_BYTES} bytes")
    return bytes(payload)


def _text_files(root: Path, generated_roots: Sequence[str]) -> tuple[bytes, dict[str, Any]]:
    listed = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True, check=False)
    if listed.returncode != 0:
        raise CampaignError(f"cannot enumerate pinned source files: {listed.stderr.decode(errors='replace').strip()}")
    generated = tuple(PurePosixPath(item) for item in generated_roots)
    payload = bytearray()
    skipped_binary = 0
    source_bytes = 0
    scanned_files = 0
    for raw_name in listed.stdout.split(b"\0"):
        if not raw_name:
            continue
        relative = raw_name.decode("utf-8")
        relative_path = PurePosixPath(relative)
        if any(relative_path == root_path or root_path in relative_path.parents for root_path in generated):
            continue
        path = root.joinpath(*relative_path.parts)
        resolved = path.resolve(strict=True)
        if not resolved.is_relative_to(root.resolve(strict=True)) or not resolved.is_file():
            raise CampaignError(f"tracked source path is not a contained regular file: {relative}")
        content = resolved.read_bytes()
        if b"\0" in content[:8192]:
            skipped_binary += 1
            continue
        try:
            content.decode("utf-8")
        except UnicodeDecodeError:
            skipped_binary += 1
            continue
        header = f"\nFILE {relative} {len(content)}\n".encode("utf-8")
        if len(payload) + len(header) + len(content) + 1 > MAX_SOURCE_AUDIT_BYTES:
            raise CampaignError(f"outgoing source text exceeds {MAX_SOURCE_AUDIT_BYTES} bytes")
        source_bytes += len(content)
        scanned_files += 1
        payload.extend(header)
        payload.extend(content)
        payload.extend(b"\n")
    return bytes(payload), {"files_scanned": scanned_files, "binary_files_skipped": skipped_binary, "source_bytes": source_bytes}


def _source_audit(project: Path, case: Mapping[str, Any], seed_digest: str, seed_project: Path) -> tuple[dict[str, Any], bytes]:
    payload, counts = _text_files(project, case["generated_roots"])
    try:
        redacted = comparison_campaign.redact_bytes(payload)
        redactor_error = None
    except Exception as error:
        redacted = b""
        redactor_error = f"{type(error).__name__}: {error}"
    seed_unchanged = edit_snapshot.snapshot_tree(seed_project, excluded=case["generated_roots"])["digest"] == seed_digest
    return ({
        **counts,
        "source_sha256": _sha256_bytes(payload),
        "redacted_sha256": _sha256_bytes(redacted),
        "redactor_changed": redactor_error is None and redacted != payload,
        "redactor_status": "complete" if redactor_error is None else "unavailable",
        "redactor_error": redactor_error,
        "seed_unchanged": seed_unchanged,
        "seed_digest": seed_digest,
    }, payload)


def _secret_environment_matches(payload: bytes, environment: Mapping[str, str] | None = None) -> list[str]:
    values = os.environ if environment is None else environment
    matches = []
    for name, value in values.items():
        upper = name.upper()
        if not value or not any(marker in upper for marker in SECRET_ENV_MARKERS):
            continue
        try:
            encoded = value.encode("utf-8")
        except UnicodeEncodeError:
            continue
        if encoded and encoded in payload:
            matches.append(name)
    return sorted(set(matches))


def _make_prompt(case: Mapping[str, Any], dependencies: Mapping[str, Path], tools: Mapping[str, Path], request_text: str, arm: str) -> str:
    public_commands = [
        {"id": command["id"], "argv": command["argv"], "cwd": command["cwd"], "timeout_seconds": command["timeout_seconds"]}
        for command in case["public_commands"]
    ]
    available = {
        "dependencies": {name: f"/deps/{name}" for name in sorted(dependencies)},
        "tools": {name: f"/tools/{name}" for name in sorted(tools)},
    }
    if arm == "code-kb":
        available["tools"]["intelligence"] = "/tools/intelligence"
    return (
        "Implement the task in /workspace/project. Preserve canonical tests, instructions, build files, dependency locks, and Git metadata. "
        "Prepared dependencies and named tools are available at the listed container paths. Run relevant public checks when useful.\n\n"
        f"Task request:\n{request_text}\n\n"
        f"Public checks:\n{json.dumps(public_commands, ensure_ascii=False, sort_keys=True)}\n\n"
        f"Prepared inputs:\n{json.dumps(available, ensure_ascii=False, sort_keys=True)}"
    )


def _trace_redaction(raw: bytes) -> tuple[bytes | None, dict[str, Any]]:
    try:
        cleaned = comparison_campaign.redact_bytes(raw)
    except Exception as error:
        return None, {"status": "unavailable", "error": f"{type(error).__name__}: {error}"}
    return cleaned, {
        "status": "complete",
        "original_sha256": _sha256_bytes(raw),
        "redacted_sha256": _sha256_bytes(cleaned),
        "changed": cleaned != raw,
        "original_bytes": len(raw),
        "redacted_bytes": len(cleaned),
    }


def _redact_trace_value(value: Any, field_name: str | None = None) -> tuple[Any, bool]:
    compact_field = re.sub(r"[^a-z0-9]", "", field_name.lower()) if isinstance(field_name, str) else ""
    if isinstance(value, str) and any(marker in compact_field for marker in TRACE_SECRET_FIELDS) and value:
        return "<REDACTED>", True
    if isinstance(value, str):
        redacted = comparison_campaign.redact_bytes(value.encode("utf-8")).decode("utf-8")
        return redacted, redacted != value
    if isinstance(value, list):
        items = [_redact_trace_value(item, field_name) for item in value]
        return [item for item, _ in items], any(changed for _, changed in items)
    if isinstance(value, dict):
        items = {key: _redact_trace_value(item, key) for key, item in value.items()}
        return {key: item for key, (item, _) in items.items()}, any(changed for _, changed in items.values())
    return value, False


def _redact_jsonl(raw: bytes) -> tuple[bytes, bytes, bool]:
    retained_events = []
    malformed_lines = []
    changed = False
    for line in raw.splitlines():
        if not line.strip():
            changed = True
            continue
        try:
            event = json.loads(line)
        except (UnicodeError, json.JSONDecodeError):
            cleaned = comparison_campaign.redact_bytes(line)
            malformed_lines.append(cleaned)
            changed = changed or cleaned != line
            continue
        cleaned, value_changed = _redact_trace_value(event)
        changed = changed or value_changed
        retained_events.append(json.dumps(cleaned, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8"))
    retained = b"\n".join(retained_events) + (b"\n" if retained_events else b"")
    malformed = b"\n".join(malformed_lines) + (b"\n" if malformed_lines else b"")
    return retained, malformed, changed


def _safe_trace(raw_path: Path, retained_path: Path, *, jsonl: bool = False) -> dict[str, Any]:
    if not raw_path.is_file():
        return {"status": "missing", "path": str(retained_path)}
    raw = raw_path.read_bytes()
    malformed = b""
    try:
        if jsonl:
            cleaned, malformed, value_changed = _redact_jsonl(raw)
            metadata = {
                "status": "partial" if malformed else "complete",
                "original_sha256": _sha256_bytes(raw),
                "redacted_sha256": _sha256_bytes(cleaned),
                "changed": value_changed,
                "normalized_bytes_changed": cleaned != raw,
                "original_bytes": len(raw),
                "redacted_bytes": len(cleaned),
                "malformed_lines": len(malformed.splitlines()),
            }
        else:
            cleaned, metadata = _trace_redaction(raw)
    except Exception as error:
        cleaned = None
        metadata = {"status": "unavailable", "error": f"{type(error).__name__}: {error}"}
    if cleaned is None:
        retained_path.unlink(missing_ok=True)
        retained_path.with_name(retained_path.name + ".malformed").unlink(missing_ok=True)
        return {**metadata, "path": str(retained_path)}
    retained_path.parent.mkdir(parents=True, exist_ok=True)
    retained_path.write_bytes(cleaned)
    retained_path.chmod(0o600)
    if malformed:
        malformed_path = retained_path.with_name(retained_path.name + ".malformed")
        malformed_path.write_bytes(malformed)
        malformed_path.chmod(0o600)
        metadata["malformed_path"] = str(malformed_path)
        metadata["malformed_sha256"] = _sha256_bytes(malformed)
    return {**metadata, "path": str(retained_path)}


def _sanitize_command_records(records: list[dict[str, Any]], evidence_root: Path, prefix: str) -> list[dict[str, Any]]:
    for index, record in enumerate(records):
        for field in ("stdout_log", "stderr_log"):
            if not record.get(field):
                continue
            raw_path = Path(record[field])
            safe_path = evidence_root / f"{prefix}-{index:02d}-{field}.log"
            record[field] = str(safe_path)
            record.setdefault("retained_traces", {})[field] = _safe_trace(raw_path, safe_path)
            raw_path.unlink(missing_ok=True)
    return records


def _sanitize_grade_logs(grade: dict[str, Any], evidence_root: Path) -> None:
    checks = grade.get("checks", {})
    if not isinstance(checks, dict):
        return
    counter = 0
    for check in checks.values():
        if not isinstance(check, dict):
            continue
        commands = check.get("commands", [])
        if not isinstance(commands, list):
            continue
        for command in commands:
            if not isinstance(command, dict):
                continue
            for field in ("stdout_log", "stderr_log"):
                if not command.get(field):
                    continue
                raw_path = Path(command[field])
                safe_path = evidence_root / f"grade-{counter:03d}-{field}.log"
                command[field] = str(safe_path)
                command.setdefault("retained_traces", {})[field] = _safe_trace(raw_path, safe_path)
                raw_path.unlink(missing_ok=True)
                counter += 1


def _preindex_code_kb(
    project: Path,
    state: Path,
    evidence: Path,
    case: Mapping[str, Any],
    seed_digest: str,
    intelligence: Path,
    timeout: float,
) -> dict[str, Any]:
    result: dict[str, Any] = {
        "status": "failed",
        "elapsed_seconds": None,
        "source_digest_before": None,
        "source_digest_after": None,
    }
    if ".code-kb" not in case["generated_roots"]:
        return {**result, "failure": "case must declare .code-kb as a generated root"}
    if _has_code_kb_index(project):
        return {**result, "failure": "agent project contains a preexisting code-kb index"}
    try:
        source_before = edit_snapshot.snapshot_tree(project, excluded=case["generated_roots"])["digest"]
    except Exception as error:
        return {**result, "failure": f"cannot snapshot project before preindex: {type(error).__name__}: {error}"}
    result["source_digest_before"] = source_before
    if source_before != seed_digest:
        return {**result, "failure": "agent project changed before code-kb preindex"}
    output = evidence / "preindex-raw"
    command_error = None
    execution = None
    try:
        execution = edit_sandbox.run(
            project,
            state,
            output,
            timeout,
            "grade",
            ["/usr/bin/env", "JULIE_EXTRACT_BIN=/tools/intelligence/julie-extract", "/tools/intelligence/code-kb", "--root", "/workspace/project", "scan"],
            tools={"intelligence": intelligence},
            protect=edit_evaluation._existing_protection(project, case),
            network=False,
        )
    except Exception as error:
        command_error = f"{type(error).__name__}: {error}"
    stdout_path = Path(execution["stdout_log"]) if execution and execution.get("stdout_log") else output / "stdout.log"
    stderr_path = Path(execution["stderr_log"]) if execution and execution.get("stderr_log") else output / "stderr.log"
    log_root = evidence / "indexing"
    stdout_trace = _safe_trace(stdout_path, log_root / "stdout.log")
    stderr_trace = _safe_trace(stderr_path, log_root / "stderr.log")
    for path in (stdout_path, stderr_path):
        path.unlink(missing_ok=True)
    shutil.rmtree(output, ignore_errors=True)
    try:
        source_after = edit_snapshot.snapshot_tree(project, excluded=case["generated_roots"])["digest"]
    except Exception as error:
        source_after = None
        command_error = command_error or f"cannot snapshot project after preindex: {type(error).__name__}: {error}"
    result.update({
        "elapsed_seconds": execution.get("elapsed_seconds") if execution else None,
        "exit_status": execution.get("exit_status") if execution else None,
        "timed_out": execution.get("timed_out") if execution else False,
        "process_cleanup": execution.get("process_cleanup") if execution else None,
        "stdout_log": stdout_trace.get("path"),
        "stderr_log": stderr_trace.get("path"),
        "stdout_redaction": stdout_trace,
        "stderr_redaction": stderr_trace,
        "source_digest_after": source_after,
        "error": command_error,
    })
    index_root = project / ".code-kb"
    index_file = index_root / "artifact.db"
    index_exists = index_root.is_dir() and not index_root.is_symlink() and index_file.is_file() and not index_file.is_symlink()
    if command_error is None and execution and execution.get("exit_status") == 0 and not execution.get("timed_out") and index_exists and source_after == seed_digest:
        result["status"] = "completed"
    else:
        failures = []
        if command_error:
            failures.append(command_error)
        if execution and execution.get("timed_out"):
            failures.append("code-kb preindex timed out")
        elif execution and execution.get("exit_status") != 0:
            failures.append(f"code-kb scan exited with status {execution.get('exit_status')}")
        if not index_exists:
            failures.append("code-kb scan did not create .code-kb/artifact.db")
        if source_after != seed_digest:
            failures.append("code-kb scan changed source files")
        result["failure"] = "; ".join(failures) or "code-kb preindex failed"
    return result


def _attempt_timing(indexing: Mapping[str, Any], execution: Mapping[str, Any]) -> dict[str, Any]:
    agent_seconds = execution.get("elapsed_seconds")
    index_seconds = indexing.get("elapsed_seconds")
    inclusive_seconds = agent_seconds
    if isinstance(agent_seconds, (int, float)) and isinstance(index_seconds, (int, float)):
        inclusive_seconds = agent_seconds + index_seconds
    return {
        "agent_wall_seconds": agent_seconds,
        "index_wall_seconds": index_seconds,
        "index_inclusive_wall_seconds": inclusive_seconds,
    }


def _turn_completed(stdout: bytes) -> bool:
    for line in stdout.splitlines():
        try:
            event = json.loads(line)
        except (UnicodeError, json.JSONDecodeError):
            continue
        if isinstance(event, dict) and event.get("type") == "turn.completed":
            return True
    return False


def _trace_metrics(stdout: bytes) -> dict[str, Any]:
    if len(stdout) > 64 * 1024 * 1024:
        return {"status": "too_large", "bytes": len(stdout)}
    events = []
    invalid_lines = 0
    for line in stdout.splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except (UnicodeError, json.JSONDecodeError):
            invalid_lines += 1
            continue
        if isinstance(event, dict):
            events.append(event)
        else:
            invalid_lines += 1
    try:
        summary = comparison_metrics.summarize_trace(events)
    except Exception:
        return {"status": "unavailable", "event_count": len(events), "invalid_json_lines": invalid_lines}
    mcp_calls = summary.get("mcp_calls")
    commands = summary.get("commands")
    errors = summary.get("errors")
    if not isinstance(mcp_calls, list) or not isinstance(commands, list) or not isinstance(errors, list):
        return {"status": "invalid_summary", "event_count": len(events), "invalid_json_lines": invalid_lines}
    try:
        safe_mcp_calls, _ = _redact_trace_value(mcp_calls)
        safe_commands, _ = _redact_trace_value(commands)
    except Exception:
        return {"status": "unavailable", "event_count": len(events), "invalid_json_lines": invalid_lines}
    return {
        "status": "partial" if invalid_lines else "complete",
        "input_tokens": summary.get("input_tokens"),
        "output_tokens": summary.get("output_tokens"),
        "cache_tokens": summary.get("cache_tokens"),
        "tool_calls": summary.get("tool_calls"),
        "code_kb_calls": summary.get("code_kb_calls"),
        "mcp_call_count": len(mcp_calls),
        "mcp_calls": safe_mcp_calls,
        "commands": safe_commands,
        "command_count": len(commands),
        "error_count": len(errors),
        "completed": summary.get("completed") is True,
        "event_count": len(events),
        "invalid_json_lines": invalid_lines,
    }


def _profile_config(profile_files: Sequence[tuple[str, bytes]], controls: Mapping[str, Any]) -> None:
    config = next((payload for name, payload in profile_files if name == "config.toml"), None)
    if config is None:
        raise CampaignError("frozen profile config is missing")
    try:
        import tomllib

        parsed = tomllib.loads(config.decode("utf-8"))
    except (UnicodeError, ValueError) as error:
        raise CampaignError(f"frozen profile config is invalid: {error}") from error
    if parsed.get("model") != controls.get("model") or parsed.get("model_reasoning_effort") != controls.get("effort"):
        raise CampaignError("frozen profile model or effort differs from the manifest")


def _copy_profile(profile: Path, expected_hash: str) -> tempfile.TemporaryDirectory:
    temporary = tempfile.TemporaryDirectory(prefix="completed-edit-private-profile-", dir="/tmp")
    destination = Path(temporary.name) / "profile"
    try:
        shutil.copytree(profile, destination, symlinks=False, copy_function=shutil.copy2)
        if edit_evaluation.hash_tree(destination) != expected_hash:
            raise CampaignError("private profile copy does not match the frozen profile")
        if (destination / "auth.json").exists():
            raise CampaignError("frozen profile must not contain authentication")
    except Exception:
        temporary.cleanup()
        raise
    return temporary


def _attempt_identity(manifest_hash: str, runner_hash: str, runtime_hash: str, schedule_index: int, job: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "schema": SCHEMA,
        "manifest_sha256": manifest_hash,
        "campaign_runner_sha256": runner_hash,
        "runtime_bindings_sha256": runtime_hash,
        "schedule_index": schedule_index,
        "case_id": job["case_id"],
        "arm": job["arm"],
        "repeat": job["repeat"],
    }


def _existing_slot(attempt_path: Path, identity: Mapping[str, Any]) -> dict[str, Any] | None:
    if not attempt_path.exists() and not attempt_path.is_symlink():
        return None
    if attempt_path.is_symlink() or not attempt_path.is_dir():
        raise CampaignError(f"attempt path is not a directory: {attempt_path}")
    outcome_path = attempt_path / "attempt.json"
    status_path = attempt_path / "status.json"
    if outcome_path.is_file():
        outcome = _read_json(outcome_path, "saved attempt")
        if any(outcome.get(key) != value for key, value in identity.items()):
            raise CampaignError(f"saved attempt identity mismatch: {attempt_path.name}")
        return {"schedule_index": identity["schedule_index"], "attempt_path": str(attempt_path), "slot_state": "existing", "outcome": outcome}
    if status_path.is_file():
        status = _read_json(status_path, "saved attempt status")
        if any(status.get(key) != value for key, value in identity.items()):
            raise CampaignError(f"saved attempt status identity mismatch: {attempt_path.name}")
        return {"schedule_index": identity["schedule_index"], "attempt_path": str(attempt_path), "slot_state": "in_progress", "status": status}
    raise CampaignError(f"attempt directory exists without a host status record: {attempt_path}")


def _write_terminal(attempt_path: Path, identity: Mapping[str, Any], outcome: dict[str, Any]) -> dict[str, Any]:
    _atomic_json(attempt_path / "attempt.json", {**identity, **outcome})
    _atomic_json(attempt_path / "status.json", {**identity, "state": "finished", "finished_at": _timestamp(), "outcome_sha256": _sha256_file(attempt_path / "attempt.json")})
    return {"schedule_index": identity["schedule_index"], "attempt_path": str(attempt_path), "slot_state": "dispatched" if outcome.get("dispatch_started") else "finished", "outcome": {**identity, **outcome}}


def _finish_preflight(attempt_path: Path, identity: Mapping[str, Any], status: str, details: Mapping[str, Any]) -> dict[str, Any]:
    outcome = {"status": status, "dispatch_started": False, "trial_completed": False, "source_review": "pending", "acceptance_status": "not_dispatched", "success": False, **details}
    return _write_terminal(attempt_path, identity, outcome)


def _slot_path(output_root: Path, schedule_index: int, job: Mapping[str, Any]) -> Path:
    arm = job["arm"].replace("/", "_")
    return output_root / "attempts" / f"{schedule_index:03d}-{job['case_id']}-{arm}-r{job['repeat']}"


def _verify_intelligence(artifact_paths: Mapping[str, Path], bindings: Mapping[str, Any]) -> Path:
    intelligence = _resolve_path(bindings.get("intelligence_tools", ""), "intelligence tools")
    if not intelligence.is_dir():
        raise CampaignError("intelligence tools must be a directory")
    if {item.name for item in intelligence.iterdir()} != {"code-kb", "julie-extract"}:
        raise CampaignError("intelligence tools directory contains unbound entries")
    for name, artifact_name in (("code-kb", "binary"), ("julie-extract", "extractor")):
        tool = intelligence / name
        if tool.is_symlink() or not tool.is_file() or not os.access(tool, os.X_OK):
            raise CampaignError(f"intelligence directory is missing executable {name}")
        if _sha256_file(tool) != _sha256_file(artifact_paths[artifact_name]):
            raise CampaignError(f"mounted intelligence {name} does not match its frozen artifact")
    return intelligence


def _verify_codex_artifacts(artifact_paths: Mapping[str, Path]) -> tuple[Path, Path]:
    codex = artifact_paths["codex"]
    companion = artifact_paths["codex_code_mode_host"]
    if codex.name != "codex" or not codex.is_file() or not os.access(codex, os.X_OK):
        raise CampaignError("frozen Codex artifact must be an executable file named codex")
    if companion.name != "codex-code-mode-host" or not companion.is_file() or not os.access(companion, os.X_OK):
        raise CampaignError("frozen Codex code-mode companion must be an executable named codex-code-mode-host")
    with codex.open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            raise CampaignError("frozen Codex artifact must be a native ELF executable")
    with companion.open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            raise CampaignError("Codex code-mode companion must be a native ELF executable")
    return codex, companion


def _assert_disjoint_mounts(dependencies: Mapping[str, Path], tools: Mapping[str, Path], codex: Path, companion: Path, intelligence: Path | None) -> None:
    mounts = [(f"dependency:{name}", path) for name, path in dependencies.items()]
    mounts.extend((f"tool:{name}", path) for name, path in tools.items())
    mounts.extend((("tool:codex", codex), ("tool:codex-code-mode-host", companion)))
    if intelligence is not None:
        mounts.append(("tool:intelligence", intelligence))
    for index, (left_name, left) in enumerate(mounts):
        for right_name, right in mounts[index + 1:]:
            if left == right or left.is_relative_to(right) or right.is_relative_to(left):
                raise CampaignError(f"sandbox mount paths overlap: {left_name} and {right_name}")


def _run_slot(
    *,
    schedule_index: int,
    job: Mapping[str, Any],
    case_item: Mapping[str, Any],
    case_dir: Path,
    case: Mapping[str, Any],
    source_root: Path,
    durable_seed: Path,
    seed_digest: str,
    runtime_case: Mapping[str, Any],
    artifacts: Mapping[str, Path],
    intelligence: Path | None,
    controls: Mapping[str, Any],
    manifest_hash: str,
    runner_hash: str,
    runtime_hash: str,
    output_root: Path,
    auth_file: Path,
) -> dict[str, Any]:
    identity = _attempt_identity(manifest_hash, runner_hash, runtime_hash, schedule_index, job)
    attempt_path = _slot_path(output_root, schedule_index, job)
    previous = _existing_slot(attempt_path, identity)
    if previous is not None:
        return previous
    attempt_path.mkdir(parents=True)
    status_path = attempt_path / "status.json"
    _atomic_json(status_path, {**identity, "state": "preparing", "started_at": _timestamp(), "dispatch_may_occur": False})
    work = attempt_path / "work"
    evidence = attempt_path / "evidence"
    work.mkdir()
    evidence.mkdir()
    try:
        case_validated = edit_evaluation.validate_case(case_dir, corpus_path=artifacts["corpus"])
        if case_validated["id"] != case["id"] or case_validated["source_pin"] != case_item["pin"]:
            raise CampaignError(f"case contract changed after manifest validation: {case['id']}")
        dependencies, case_tools = _named_inputs(case_validated, runtime_case)
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "case_preflight_failed", {"failure": f"{type(error).__name__}: {error}"})
    seed_copy = work / "seed-project"
    try:
        shutil.copytree(durable_seed, seed_copy, symlinks=True, copy_function=shutil.copy2)
        seed_snapshot = edit_snapshot.snapshot_tree(seed_copy, excluded=case_validated["generated_roots"])
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "seed_copy_failed", {"failure": f"{type(error).__name__}: {error}", "seed_digest": seed_digest})
    if seed_snapshot["digest"] != seed_digest:
        return _finish_preflight(attempt_path, identity, "seed_copy_mismatch", {"seed_digest": seed_digest, "seed_copy_digest": seed_snapshot["digest"]})
    if _has_code_kb_index(seed_copy):
        return _finish_preflight(attempt_path, identity, "preexisting_index", {"failure": "seed copy contains a preexisting code-kb index"})
    for hidden in case_validated["hidden_files"]:
        hidden_path = seed_copy.joinpath(*PurePosixPath(hidden["path"]).parts)
        if hidden_path.exists() or hidden_path.is_symlink():
            return _finish_preflight(attempt_path, identity, "hidden_input_present", {"hidden_absent": False})
    agent_project = work / "agent-project"
    try:
        shutil.copytree(seed_copy, agent_project, symlinks=True, copy_function=shutil.copy2)
        agent_seed_digest = edit_snapshot.snapshot_tree(agent_project, excluded=case_validated["generated_roots"])["digest"]
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "agent_copy_failed", {"failure": f"{type(error).__name__}: {error}", "seed_digest": seed_digest})
    if agent_seed_digest != seed_digest:
        return _finish_preflight(attempt_path, identity, "agent_seed_mismatch", {"seed_digest": seed_digest})
    if _has_code_kb_index(agent_project):
        return _finish_preflight(attempt_path, identity, "preexisting_index", {"failure": "agent project contains a preexisting code-kb index"})
    try:
        runtime_setup = edit_evaluation._prepared_runtime_state(agent_project, work, evidence, case_validated, dependencies, case_tools, "agent")
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "runtime_setup_failed", {"failure": f"{type(error).__name__}: {error}", "seed_digest": seed_digest})
    _sanitize_command_records(runtime_setup, evidence / "runtime", "setup")
    if runtime_setup and not edit_evaluation._commands_pass(runtime_setup):
        return _finish_preflight(attempt_path, identity, "runtime_setup_failed", {"runtime_setup": runtime_setup, "seed_digest": seed_digest})
    try:
        setup_digest = edit_snapshot.snapshot_tree(agent_project, excluded=case_validated["generated_roots"])["digest"]
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "runtime_snapshot_failed", {"failure": f"{type(error).__name__}: {error}", "runtime_setup": runtime_setup})
    if setup_digest != seed_digest:
        return _finish_preflight(attempt_path, identity, "runtime_changed_source", {"runtime_setup": runtime_setup, "seed_digest": seed_digest, "project_digest": setup_digest})
    indexing: dict[str, Any] = {"status": "not_applicable", "elapsed_seconds": None}
    if _has_code_kb_index(agent_project):
        indexing = {"status": "not_started", "elapsed_seconds": None, "failure": "runtime setup created a preexisting code-kb index"}
        return _finish_preflight(attempt_path, identity, "preexisting_index", {"failure": indexing["failure"], "indexing": indexing, "runtime_setup": runtime_setup})
    if job["arm"] == "code-kb":
        _atomic_json(status_path, {**identity, "state": "preindexing", "started_at": _timestamp(), "dispatch_may_occur": False})
        indexing = _preindex_code_kb(
            agent_project,
            work / "state-agent",
            evidence,
            case_validated,
            seed_digest,
            intelligence,
            controls["timeout_seconds"],
        )
        if indexing["status"] != "completed":
            return _finish_preflight(attempt_path, identity, "preindex_failed", {"indexing": indexing, "runtime_setup": runtime_setup})
    try:
        source_audit, source_payload = _source_audit(agent_project, case_validated, seed_digest, durable_seed)
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "source_audit_failed", {"failure": f"{type(error).__name__}: {error}"})
    if not source_audit["seed_unchanged"]:
        return _finish_preflight(attempt_path, identity, "durable_seed_changed", {"source_audit": source_audit})
    try:
        profile, profile_files = _profile_files(artifacts["profiles"], job["arm"])
        profile_root_hash = edit_evaluation.hash_tree(profile)
        _profile_config(profile_files, controls)
        profile_payload = _framed_payload(profile_files)
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "profile_preflight_failed", {"failure": f"{type(error).__name__}: {error}", "source_audit": source_audit})
    profile_redacted, profile_redaction = _trace_redaction(profile_payload)
    request_path = case_dir / case_validated["request"]
    try:
        request_bytes = request_path.read_bytes()
    except OSError as error:
        return _finish_preflight(attempt_path, identity, "request_unavailable", {"request_error": str(error), "source_audit": source_audit})
    try:
        request_text = request_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        return _finish_preflight(attempt_path, identity, "request_invalid", {"request_error": str(error), "source_audit": source_audit})
    try:
        prompt = _make_prompt(case_validated, dependencies, case_tools, request_text, job["arm"])
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "prompt_preflight_failed", {"failure": f"{type(error).__name__}: {error}", "source_audit": source_audit})
    prompt_bytes = prompt.encode("utf-8")
    prompt_redacted, prompt_redaction = _trace_redaction(prompt_bytes)
    combined_outbound = source_payload + prompt_bytes + profile_payload
    secret_matches = _secret_environment_matches(combined_outbound)
    if profile_redacted is None or prompt_redacted is None or profile_redaction["changed"] or prompt_redaction["changed"] or secret_matches:
        if prompt_redacted is not None:
            (attempt_path / "outbound_prompt.redacted.txt").write_bytes(prompt_redacted)
        return _finish_preflight(attempt_path, identity, "privacy_preflight_failed", {
            "source_audit": source_audit,
            "profile_redaction": profile_redaction,
            "prompt_redaction": prompt_redaction,
            "secret_environment_names": secret_matches,
        })
    prompt_path = attempt_path / "outbound_prompt.txt"
    prompt_path.write_bytes(prompt_bytes)
    prompt_path.chmod(0o600)
    profile_hash = profile_root_hash
    try:
        allowed, policy_note = comparison_campaign.provider_policy(agent_project)
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "policy_preflight_failed", {"failure": f"{type(error).__name__}: {error}", "source_audit": source_audit})
    if not allowed:
        return _finish_preflight(attempt_path, identity, "policy_denied", {"policy_note": policy_note, "source_audit": source_audit})
    codex_tool = artifacts["codex"]
    agent_tools = {**case_tools, "codex": codex_tool, "codex-code-mode-host": artifacts["codex_code_mode_host"]}
    if job["arm"] == "code-kb":
        agent_tools["intelligence"] = intelligence
    if source_root == agent_project or durable_seed == agent_project:
        raise CampaignError("agent project overlaps pinned source inputs")
    try:
        private_profile_context = _copy_profile(profile, profile_root_hash)
    except Exception as error:
        return _finish_preflight(attempt_path, identity, "profile_copy_failed", {"failure": f"{type(error).__name__}: {error}", "source_audit": source_audit})
    try:
        private_profile = Path(private_profile_context.name) / "profile"
        agent_argv = ["codex", "exec", "--json", "--ephemeral", "--ignore-rules", "--color", "never", "-o", "/state/final.txt", prompt]
        command_hash = _sha256_bytes(json.dumps(agent_argv, ensure_ascii=False, separators=(",", ":")).encode("utf-8"))
        _atomic_json(status_path, {
            **identity,
            "state": "dispatching",
            "started_at": _timestamp(),
            "dispatch_may_occur": True,
            "prompt_sha256": _sha256_bytes(prompt_bytes),
            "profile_sha256": profile_hash,
            "command_sha256": command_hash,
            "source_seed_digest": seed_digest,
        })
        raw_execution = None
        execution_error = None
        with tempfile.TemporaryDirectory(prefix="completed-edit-agent-log-") as temporary_logs:
            raw_output = Path(temporary_logs) / "codex-output"
            try:
                raw_execution = edit_sandbox.run(
                    agent_project,
                    work / "state-agent",
                    raw_output,
                    controls["timeout_seconds"],
                    "agent",
                    agent_argv,
                    dependencies=dependencies,
                    tools=agent_tools,
                    protect=edit_evaluation._existing_protection(agent_project, case_validated),
                    network=False,
                    private_profile=private_profile,
                    auth_file=auth_file,
                )
            except Exception as error:
                execution_error = f"{type(error).__name__}: {error}"
            raw_stdout_path = Path(raw_execution["stdout_log"]) if raw_execution and raw_execution.get("stdout_log") else raw_output / "stdout.log"
            raw_stderr_path = Path(raw_execution["stderr_log"]) if raw_execution and raw_execution.get("stderr_log") else raw_output / "stderr.log"
            raw_stdout = raw_stdout_path.read_bytes() if raw_stdout_path.is_file() else b""
            turn_completed = _turn_completed(raw_stdout)
            agent_evidence = evidence / "agent"
            stdout_trace = _safe_trace(raw_stdout_path, agent_evidence / "stdout.log", jsonl=True)
            stderr_trace = _safe_trace(raw_stderr_path, agent_evidence / "stderr.log")
            final_path = work / "state-agent" / "final.txt"
            answer_trace = _safe_trace(final_path, evidence / "answer.txt")
            if answer_trace.get("status") == "complete" and final_path.is_file():
                final_path.write_bytes((evidence / "answer.txt").read_bytes())
        if raw_execution is None:
            raw_execution = {"exit_status": None, "timed_out": False, "elapsed_seconds": None, "process_cleanup": {"attempted": False, "complete": False, "signal": None}, "network_scope": "disabled"}
        execution = {
            **raw_execution,
            "stdout_log": stdout_trace.get("path"),
            "stderr_log": stderr_trace.get("path"),
            "stdout_redaction": stdout_trace,
            "stderr_redaction": stderr_trace,
            "answer_path": answer_trace.get("path") if answer_trace.get("status") == "complete" else None,
            "answer_redaction": answer_trace,
            "error": execution_error,
            "turn_completed_event": turn_completed,
        }
        trace_metrics = _trace_metrics(raw_stdout)
        completed = bool(raw_execution.get("exit_status") == 0 and not raw_execution.get("timed_out") and turn_completed and execution_error is None)
        try:
            seed_after_agent = edit_snapshot.snapshot_tree(seed_copy, excluded=case_validated["generated_roots"])["digest"]
            durable_after_agent = edit_snapshot.snapshot_tree(durable_seed, excluded=case_validated["generated_roots"])["digest"]
            seed_unchanged = seed_after_agent == seed_digest and durable_after_agent == seed_digest
        except Exception:
            seed_unchanged = False
        bundle_path = attempt_path / "bundle"
        try:
            bundle = edit_snapshot.capture_bundle(seed_snapshot, agent_project, bundle_path, protected=case_validated["protected_paths"], excluded=case_validated["generated_roots"])
        except Exception as error:
            return _write_terminal(attempt_path, identity, {
                "status": "capture_failed",
                "dispatch_started": True,
                "trial_completed": completed,
                "source_review": "pending",
                "acceptance_status": "grading_failed",
                "success": False,
                "execution": execution,
                "indexing": indexing,
                "timing": _attempt_timing(indexing, raw_execution),
                "trace_metrics": trace_metrics,
                "runtime_setup": runtime_setup,
                "source_audit": {**source_audit, "seed_unchanged": seed_unchanged},
                "capture_error": f"{type(error).__name__}: {error}",
                "agent_project": str(agent_project),
                "seed_project": str(seed_copy),
            })
        grade_path = attempt_path / "grade"
        try:
            grade = edit_evaluation.replay_and_grade(
                case_dir,
                seed_copy,
                bundle_path,
                grade_path,
                tools=case_tools,
                dependencies=dependencies,
                trial_completed=completed,
                corpus_path=artifacts["corpus"],
            )
        except Exception as error:
            grade = {"behavioral_pass": False, "source_review": "pending", "acceptance_status": "grading_failed", "success": False, "error": f"{type(error).__name__}: {error}"}
        _sanitize_grade_logs(grade, evidence / "grade")
        if (grade_path / "grade.json").is_file():
            _atomic_json(grade_path / "grade.json", grade)
        status = "completed" if completed else "timed_out" if raw_execution.get("timed_out") else "incomplete"
        if execution_error:
            status = "execution_error"
        outcome = {
            "status": status,
            "dispatch_started": True,
            "trial_completed": completed,
            "source_review": grade.get("source_review", "pending"),
            "acceptance_status": grade.get("acceptance_status", "failed"),
            "success": False,
            "model": controls["model"],
            "effort": controls["effort"],
            "provider_policy": policy_note,
            "runtime_setup": runtime_setup,
            "source_audit": {**source_audit, "seed_unchanged": seed_unchanged},
            "prompt_sha256": _sha256_bytes(prompt_bytes),
            "profile_sha256": profile_hash,
            "agent_command_sha256": command_hash,
            "execution": execution,
            "indexing": indexing,
            "timing": _attempt_timing(indexing, raw_execution),
            "trace_metrics": trace_metrics,
            "bundle": bundle,
            "bundle_path": str(bundle_path),
            "bundle_json_sha256": _sha256_file(bundle_path / "bundle.json"),
            "seed_project": str(seed_copy),
            "agent_project": str(agent_project),
            "grade_path": str(grade_path),
            "grade": grade,
            "seed_unchanged": seed_unchanged,
            "protection_status": "violation" if bundle["violations"] else "unchanged",
        }
        return _write_terminal(attempt_path, identity, outcome)
    finally:
        private_profile_context.cleanup()


def run_campaign(
    manifest_path: str | Path,
    artifact_paths: Mapping[str, str | Path],
    runtime_bindings_path: str | Path,
    output_dir: str | Path,
    auth_file: str | Path,
    *,
    schedule_indices: Sequence[int] | None = None,
) -> dict[str, Any]:
    """Run selected frozen schedule slots once, preserving every attempted patch."""
    manifest = _resolve_path(manifest_path, "campaign manifest")
    bindings_file = _resolve_path(runtime_bindings_path, "runtime bindings")
    output = Path(output_dir).expanduser()
    if not output.is_absolute() or output.is_symlink():
        raise CampaignError("campaign output must be an absolute non-symbolic path")
    output = output.resolve(strict=False)
    artifacts = _artifact_map(artifact_paths)
    required_artifacts = {"codex", "codex_code_mode_host", "profiles", "corpus", "binary", "extractor", "trace_metrics"}
    required_artifacts.add("campaign_runner")
    missing_artifacts = required_artifacts - artifacts.keys()
    if missing_artifacts:
        raise CampaignError(f"artifact paths are missing: {', '.join(sorted(missing_artifacts))}")
    auth = _validate_auth_file(auth_file)
    runtime_hash = _sha256_file(bindings_file)
    bindings = _read_json(bindings_file, "runtime bindings")
    if _sha256_file(bindings_file) != runtime_hash:
        raise CampaignError("runtime bindings changed while they were being read")
    _validate_runtime_bindings(bindings)
    try:
        validation = edit_evaluation.validate_manifest(manifest, corpus_path=artifacts["corpus"], artifact_paths=artifacts)
    except (edit_evaluation.EvaluationError, OSError, ValueError) as error:
        raise CampaignError(f"frozen campaign validation failed: {error}") from error
    if validation.get("valid") is not True:
        raise CampaignError("frozen campaign validation did not succeed")
    manifest_data = _read_json(manifest, "campaign manifest")
    manifest_hash = _sha256_file(manifest)
    runner_hash = _sha256_file(Path(__file__).resolve(strict=True))
    if artifacts["campaign_runner"] != Path(__file__).resolve(strict=True) or _sha256_file(artifacts["campaign_runner"]) != runner_hash:
        raise CampaignError("campaign runner artifact must match the active launcher source")
    metrics_source = Path(comparison_metrics.__file__).resolve(strict=True)
    if artifacts["trace_metrics"] != metrics_source or _sha256_file(artifacts["trace_metrics"]) != _sha256_file(metrics_source):
        raise CampaignError("trace metrics artifact must match the active comparison_metrics source")
    cases = manifest_data.get("cases")
    schedule = manifest_data.get("schedule")
    controls = manifest_data.get("controls")
    if not isinstance(cases, list) or not isinstance(schedule, list) or not isinstance(controls, dict):
        raise CampaignError("campaign manifest is missing cases, schedule, or controls")
    if "timeout_seconds" not in controls or "model" not in controls or "effort" not in controls:
        raise CampaignError("campaign controls are missing model, effort, or timeout")
    case_items = {item["id"]: item for item in cases}
    if len(case_items) != len(cases):
        raise CampaignError("campaign manifest contains duplicate case ids")
    if schedule_indices is None:
        selected = list(range(1, len(schedule) + 1))
    else:
        selected = list(schedule_indices)
        if any(isinstance(index, bool) or not isinstance(index, int) or index < 1 or index > len(schedule) for index in selected):
            raise CampaignError(f"schedule indices must be between 1 and {len(schedule)}")
        if len(selected) != len(set(selected)):
            raise CampaignError("schedule indices must not be duplicated")
    runtime_cases = bindings["cases"]
    codex_tool, codex_companion = _verify_codex_artifacts(artifacts)
    validated_cases = {}
    seeds = {}
    source_roots = {}
    for case_id, item in case_items.items():
        case_dir = _contained_path(manifest.parent, item["case_path"], f"{case_id} case directory")
        if edit_evaluation.hash_tree(case_dir) != item.get("case_sha256"):
            raise CampaignError(f"case tree digest is stale for {case_id}")
        case = edit_evaluation.validate_case(case_dir, corpus_path=artifacts["corpus"])
        source_root, durable_seed, seed_digest = _verify_seed(item, case, bindings, manifest.parent)
        dependencies, tools = _named_inputs(case, runtime_cases.get(case_id, {}))
        _verify_named_input_artifacts(case_id, dependencies, tools, artifacts)
        validated_cases[case_id] = case
        seeds[case_id] = (durable_seed, seed_digest)
        source_roots[case_id] = source_root
    case_directories = {case_id: _contained_path(manifest.parent, item["case_path"], f"{case_id} case directory") for case_id, item in case_items.items()}
    for input_path in (manifest, bindings_file, auth, *artifacts.values(), *(seed for seed, _ in seeds.values()), *source_roots.values(), *case_directories.values()):
        if output == input_path or output.is_relative_to(input_path) or input_path.is_relative_to(output):
            raise CampaignError(f"campaign output overlaps a frozen input: {input_path}")
    if any(not isinstance(schedule[index - 1], dict) for index in selected):
        raise CampaignError("campaign schedule contains a non-object entry")
    needs_intelligence = any(schedule[index - 1].get("arm") == "code-kb" for index in selected)
    intelligence = _verify_intelligence(artifacts, bindings) if needs_intelligence else None
    for index in selected:
        job = schedule[index - 1]
        case_id = job.get("case_id")
        if case_id not in validated_cases:
            raise CampaignError(f"schedule references an unknown case: {case_id}")
        dependencies, case_tools = _named_inputs(validated_cases[case_id], runtime_cases[case_id])
        _assert_disjoint_mounts(dependencies, case_tools, codex_tool, codex_companion, intelligence if job.get("arm") == "code-kb" else None)
    runner_record = {"schema": SCHEMA, "manifest_sha256": manifest_hash, "campaign_runner_sha256": runner_hash, "runtime_bindings_sha256": runtime_hash, "schedule_count": len(schedule)}
    if output.exists():
        if not output.is_dir():
            raise CampaignError(f"campaign output is not a directory: {output}")
        campaign_path = output / "campaign.json"
        if campaign_path.exists():
            existing = _read_json(campaign_path, "campaign output header")
            if any(existing.get(key) != value for key, value in runner_record.items()):
                raise CampaignError("campaign output belongs to a different manifest or launcher version")
        elif any(output.iterdir()):
            raise CampaignError("campaign output is non-empty and has no campaign identity record")
    else:
        output.mkdir(parents=True)
    if not (output / "campaign.json").exists():
        _atomic_json(output / "campaign.json", runner_record)
    runs = []
    for schedule_index in selected:
        job = dict(schedule[schedule_index - 1])
        case_id = job.get("case_id")
        if case_id not in validated_cases:
            raise CampaignError(f"schedule references an unknown case: {case_id}")
        if job.get("arm") not in ("native", "code-kb"):
            raise CampaignError(f"schedule contains an unsupported arm: {job.get('arm')}")
        job["schedule_index"] = schedule_index
        case = validated_cases[case_id]
        runtime_case = runtime_cases[case_id]
        durable_seed, seed_digest = seeds[case_id]
        source_root = source_roots[case_id]
        runs.append(_run_slot(
            schedule_index=schedule_index,
            job=job,
            case_item=case_items[case_id],
            case_dir=case_directories[case_id],
            case=case,
            source_root=source_root,
            durable_seed=durable_seed,
            seed_digest=seed_digest,
            runtime_case=runtime_case,
            artifacts=artifacts,
            intelligence=intelligence,
            controls=controls,
            manifest_hash=manifest_hash,
            runner_hash=runner_hash,
            runtime_hash=runtime_hash,
            output_root=output,
            auth_file=auth,
        ))
    status = "in_progress" if any(run["slot_state"] == "in_progress" for run in runs) else "finished"
    return {"status": status, "manifest_sha256": manifest_hash, "campaign_runner_sha256": runner_hash, "runtime_bindings_sha256": runtime_hash, "runs": runs}


def _load_artifact_paths(path: str | Path) -> dict[str, str]:
    value = _read_json(path, "artifact path map")
    if any(not isinstance(key, str) or not isinstance(item, str) for key, item in value.items()):
        raise CampaignError("artifact path map must map string names to string paths")
    return value


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--artifact-paths", required=True, help="Host-only JSON map of frozen artifacts to absolute paths")
    parser.add_argument("--runtime-bindings", required=True, help="Host-only JSON map of named local seeds, dependencies, and tools")
    parser.add_argument("--output", required=True)
    parser.add_argument("--auth-file", default=str(Path.home() / ".codex" / "auth.json"))
    parser.add_argument("--schedule-index", action="append", type=int, help="1-based schedule slot; may be repeated, defaults to all slots")
    return parser


def main(arguments: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(arguments)
    try:
        result = run_campaign(
            args.manifest,
            _load_artifact_paths(args.artifact_paths),
            args.runtime_bindings,
            args.output,
            args.auth_file,
            schedule_indices=args.schedule_index,
        )
    except (CampaignError, edit_evaluation.EvaluationError, edit_sandbox.SandboxError, edit_snapshot.SnapshotError, OSError, subprocess.SubprocessError) as error:
        print(json.dumps({"error": f"{type(error).__name__}: {error}"}, sort_keys=True), file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True, indent=2))
    return 1 if result["status"] == "in_progress" else 0


if __name__ == "__main__":
    sys.exit(main())
