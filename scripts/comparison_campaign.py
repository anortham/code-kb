import argparse
import csv
import hashlib
import json
import math
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

try:
    from .agent_comparison import capture, codex_command, isolate, schedule, stage_fixture
    from .comparison_metrics import grade_answer, summarize_trace
except ImportError:
    from agent_comparison import capture, codex_command, isolate, schedule, stage_fixture
    from comparison_metrics import grade_answer, summarize_trace


REVIEW = Path("evals/review-2026-10-26")
REDACTOR = "redact-outbound"
AUTH_FILE = Path("/home/murphy/.codex/auth.json")
PROFILE_TARGET = Path("/home/murphy/.codex")
HIDDEN_ROOTS = (
    Path("/home/murphy/source"),
    Path("/home/murphy/.codex/worktrees"),
    Path("/home/murphy/.agents"),
    Path("/home/murphy/.claude"),
    Path("/home/murphy/.local/bin"),
)
COMMON_INSTRUCTIONS = """This task is read-only. Do not edit, create, or delete project files, and do not run mutating commands. The arm-specific navigation policy below takes precedence over generic routing hints in MCP tool descriptions. Report every requested declaration using its terminal name and the containing type in owner (null for no owner), with a repository-relative file and line. List only selected definitions in symbols; describe excluded alternatives in explanation. Explain the source chain. For each focused test, give an exact command only when the fixture evidences one; otherwise set command to null, command_status to unavailable, and explain why."""


def parse_args(argv=None):
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description="Run the frozen agent comparison campaign.")
    parser.add_argument("--root", type=Path, default=root)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=root / "target" / "release" / "code-kb")
    parser.add_argument("--julie", type=Path, default=root / ".tools" / "julie-extract")
    parser.add_argument("--codex", type=Path, default=Path("/home/murphy/.local/share/mise/installs/node/lts/bin/codex"))
    parser.add_argument("--run", action="store_true")
    return parser.parse_args(argv)


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def frozen_files(root):
    base = Path(root)
    review = Path("evals/review-2026-10-26")
    required = {
        review / name
        for name in (
            "tasks.json",
            "oracles.json",
            "answer-schema.json",
            "tool-definitions.json",
            "guidance/native.md",
            "guidance/current.md",
            "guidance/revised.md",
        )
    }
    required.update(Path(name) for name in (
        "scripts/agent_comparison.py",
        "scripts/comparison_metrics.py",
        "scripts/comparison_campaign.py",
    ))
    fixture = base / review / "fixture"
    if not fixture.is_dir():
        raise ValueError("fixture directory is missing")
    excluded = {"target", ".code-kb", "__pycache__", ".git"}
    forbidden = {"oracles.json", "tasks.json", ".memories", "AGENTS.md", "CLAUDE.md"}
    for path in fixture.rglob("*"):
        relative = path.relative_to(fixture)
        if any(part in excluded for part in relative.parts):
            continue
        if path.is_symlink() or any(part in forbidden for part in relative.parts):
            raise ValueError(f"fixture contains a forbidden input: {relative.as_posix()}")
        if path.is_file():
            required.add(review / "fixture" / relative)
    return {path.as_posix() for path in required}


def validate_manifest(root, binary, julie):
    root = Path(root).resolve()
    path = root / "evals/review-2026-10-26/experiment.json"
    try:
        raw = path.read_bytes()
        experiment = json.loads(raw)
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read experiment manifest: {error}") from error
    if not isinstance(experiment, dict) or experiment.get("schema") != 1 or not all(experiment.get(key) for key in ("model", "effort", "harness")):
        raise ValueError("experiment manifest has an unsupported schema or missing run identity")
    files = experiment.get("files")
    expected_files = frozen_files(root)
    if not isinstance(files, dict):
        raise ValueError("manifest input set is invalid")
    if set(files) != expected_files:
        missing = sorted(expected_files - set(files))
        extra = sorted(set(files) - expected_files)
        raise ValueError(f"manifest input set drift (missing={missing}, extra={extra})")
    for relative, expected in files.items():
        if not isinstance(relative, str) or not isinstance(expected, str):
            raise ValueError("manifest input paths and hashes must be strings")
        item = root / relative
        if (
            Path(relative).is_absolute() or ".." in Path(relative).parts or "\\" in relative
            or not item.is_file() or item.is_symlink()
            or any(parent.is_symlink() for parent in item.parents if parent != root)
            or not item.resolve().is_relative_to(root)
        ):
            raise ValueError(f"manifest input is missing or unsafe: {relative}")
        actual = sha256(item)
        if actual != expected:
            raise ValueError(f"manifest hash mismatch: {relative}")
    for key, executable in (("engine_sha256", binary), ("extractor_sha256", julie)):
        try:
            actual = sha256(executable)
        except OSError as error:
            raise ValueError(f"cannot hash {key.removesuffix('_sha256')}: {error}") from error
        if actual != experiment.get(key):
            raise ValueError(f"manifest hash mismatch: {key.removesuffix('_sha256')}")
    tasks = json.loads((root / "evals/review-2026-10-26/tasks.json").read_text())
    oracles = json.loads((root / "evals/review-2026-10-26/oracles.json").read_text())
    if not isinstance(tasks, dict) or not isinstance(oracles, dict) or not isinstance(oracles.get("tasks"), dict):
        raise ValueError("task or oracle data has an invalid shape")
    task_rows = tasks.get("tasks")
    if not isinstance(task_rows, list) or any(
        not isinstance(task, dict) or not isinstance(task.get("prompt"), str)
        or not isinstance(task.get("language"), str) for task in task_rows
    ):
        raise ValueError("task entries must have language and prompt fields")
    task_ids = [task.get("id") for task in task_rows]
    if tasks.get("schema") != 1 or len(task_ids) != 8 or any(not isinstance(task_id, str) for task_id in task_ids) or len(set(task_ids)) != 8:
        raise ValueError("the frozen campaign must contain eight uniquely named tasks")
    if set(oracles.get("tasks", {})) != set(task_ids):
        raise ValueError("task and oracle identifiers do not match")
    return experiment, hashlib.sha256(raw).hexdigest(), tasks, oracles


class CampaignError(RuntimeError):
    pass


def redact_bytes(payload, redactor=None):
    if callable(redactor):
        result = redactor(payload)
        if not isinstance(result, bytes):
            raise CampaignError("outbound redaction failed")
        return result
    command = REDACTOR if redactor is None else redactor
    if not isinstance(command, (str, Path)):
        raise CampaignError("outbound redactor must be a callable or executable path")
    command = str(command)
    if not Path(command).is_absolute():
        requested = command
        command = shutil.which(requested)
        if command is None:
            raise CampaignError(f"outbound redactor `{requested}` is unavailable on PATH")
    try:
        result = subprocess.run([command], input=payload, capture_output=True, check=False)
    except OSError as error:
        raise CampaignError("outbound redaction failed") from error
    if result.returncode != 0:
        raise CampaignError("outbound redaction failed")
    return result.stdout


def outbound_payload(root, task, arm, redactor=None):
    root = Path(root)
    review = root / REVIEW
    guidance = (review / "guidance" / f"{arm}.md").read_text()
    schema = json.loads((review / "answer-schema.json").read_text())
    fixture = {}
    source = review / "fixture"
    excluded = {"target", ".code-kb", "__pycache__", ".git"}
    forbidden = {"oracles.json", "tasks.json", ".memories", "AGENTS.md", "CLAUDE.md"}
    for item in sorted(source.rglob("*")):
        relative = item.relative_to(source)
        if any(part in excluded for part in relative.parts):
            continue
        if item.is_symlink() or any(part in forbidden for part in relative.parts):
            raise CampaignError(f"fixture contains a forbidden input: {relative.as_posix()}")
        if item.is_file():
            try:
                fixture[relative.as_posix()] = item.read_text()
            except UnicodeDecodeError as error:
                raise CampaignError("fixture contains non-text content") from error
    prompt = f"Task ID: {task['id']}\nLanguage: {task['language']}\n\n{task['prompt']}"
    bundle = {"prompt": prompt, "guidance": COMMON_INSTRUCTIONS + "\n\n" + guidance, "schema": schema, "fixture": fixture}
    encoded = json.dumps(bundle, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    try:
        cleaned = json.loads(redact_bytes(encoded, redactor))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CampaignError("outbound redaction returned an invalid payload") from error
    if cleaned.get("fixture") != fixture or cleaned.get("guidance") != bundle["guidance"] or cleaned.get("schema") != schema:
        raise CampaignError("outbound redaction changed the frozen fixture or instructions")
    if not isinstance(cleaned.get("prompt"), str):
        raise CampaignError("outbound redaction changed the prompt shape")
    return (
        cleaned["prompt"], cleaned["guidance"],
        json.dumps(schema, ensure_ascii=False, sort_keys=True).encode(),
        cleaned["prompt"] != prompt,
    )


def provider_policy(root):
    blocks = []
    for name in ("AGENTS.md", "CLAUDE.md"):
        path = Path(root) / name
        if not path.is_file():
            continue
        text = path.read_text().splitlines()
        for index, line in enumerate(text):
            if not re.match(r"^#{1,6}\s+external[- ]model policy\b", line.strip(), re.IGNORECASE):
                continue
            section = []
            for following in text[index + 1:]:
                if re.match(r"^#{1,6}\s+", following.strip()):
                    break
                section.append(following)
            providers = [row.split(":", 1)[1].strip() for row in section if row.strip().lower().startswith("allowed providers:")]
            if not providers:
                blocks.append(False)
            else:
                for value in providers:
                    names = {part.strip().lower() for part in value.split(",")}
                    known = {"anthropic", "openai", "xai", "cursor", "google", "any"}
                    blocks.append(names == {"any"} or ("openai" in names and names <= known))
    if not blocks:
        return True, "no external-model policy declared — synthetic fixture sent to openai"
    if not all(blocks):
        return False, "project policy does not allow openai"
    return True, "project policy allows openai"


def _git(args, cwd=None):
    env = os.environ.copy()
    env.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
    try:
        result = __import__("subprocess").run(
            ["git", *args], cwd=cwd, env=env, capture_output=True, text=True, timeout=30, check=False
        )
    except (OSError, TimeoutError) as error:
        raise CampaignError("local fixture Git operation failed") from error
    if result.returncode:
        raise CampaignError("local fixture Git operation failed")
    return result.stdout.strip()


def prepare_seed(root, output):
    root, output = Path(root), Path(output)
    seed, commit_path = output / "seed", output / "seed-commit.txt"
    if seed.exists() or commit_path.exists():
        if not seed.is_dir() or not commit_path.is_file():
            raise CampaignError("saved seed state is incomplete; refusing to replace it")
        commit = commit_path.read_text().strip()
        if _git(["-C", str(seed), "rev-parse", "HEAD"]) != commit:
            raise CampaignError("saved seed commit changed")
        if _git(["-C", str(seed), "remote"]) or _git(["-C", str(seed), "status", "--porcelain"]):
            raise CampaignError("saved seed is not a clean local repository")
        return seed, commit
    stage = Path(tempfile.mkdtemp(prefix=".comparison-seed-", dir=output))
    shutil.rmtree(stage)
    try:
        stage_fixture(root / REVIEW / "fixture", stage)
        _git(["init", "--quiet", "--initial-branch=seed"], cwd=stage)
        _git(["add", "--all"], cwd=stage)
        env = os.environ.copy()
        env.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
        import subprocess

        result = subprocess.run(
            ["git", "-C", str(stage), "-c", "user.name=Code KB Comparison", "-c", "user.email=comparison@localhost", "commit", "--quiet", "-m", "fixture seed"],
            env=env, capture_output=True, text=True, timeout=30, check=False,
        )
        if result.returncode:
            raise CampaignError("local fixture seed commit failed")
        commit = _git(["-C", str(stage), "rev-parse", "HEAD"])
        if _git(["-C", str(stage), "remote"]):
            raise CampaignError("fixture seed unexpectedly has a remote")
        stage.rename(seed)
        commit_path.write_text(commit + "\n")
        return seed, commit
    except Exception:
        if stage.exists():
            shutil.rmtree(stage)
        raise


def _read_trace(path):
    events, invalid = [], 0
    if path.is_file():
        for line in path.read_text(errors="replace").splitlines():
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                invalid += 1
    return events, invalid


TELEMETRY_COLUMNS = (
    "tool", "duration_ms", "outcome", "est_tokens", "est_tokens_saved",
    "est_tokens_saved_known", "result_count", "result_count_known",
)


def _read_telemetry(telemetry_dir):
    database = Path(telemetry_dir) / "telemetry.db"
    if not database.is_file():
        return None, None
    connection = None
    try:
        connection = sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True)
        connection.execute("PRAGMA query_only=ON")
        columns = ", ".join(TELEMETRY_COLUMNS)
        rows = [dict(zip(TELEMETRY_COLUMNS, row)) for row in connection.execute(
            f"SELECT {columns} FROM tool_telemetry ORDER BY rowid"
        )]
        return rows, None
    except sqlite3.Error as error:
        return None, str(error)
    finally:
        if connection is not None:
            connection.close()


def _latency_summary(rows):
    values = sorted(
        row["duration_ms"] for row in rows
        if isinstance(row.get("duration_ms"), (int, float))
        and not isinstance(row["duration_ms"], bool)
        and math.isfinite(row["duration_ms"])
    )
    if not values:
        return {"samples": 0, "p50_ms": None, "p95_ms": None}

    def nearest_rank(percentile):
        value = values[max(0, math.ceil(percentile * len(values)) - 1)]
        return int(value) if isinstance(value, float) and value.is_integer() else value

    return {"samples": len(values), "p50_ms": nearest_rank(0.5), "p95_ms": nearest_rank(0.95)}


def _atomic_json(path, value, redactor=None):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    data = json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2).encode() + b"\n"
    safe = redact_bytes(data, redactor)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_bytes(safe)
    temporary.replace(path)


def _record_outcome(run_dir, attempt_dir, outcome, artifacts, redactor):
    try:
        for relative, content in artifacts.items():
            target = attempt_dir / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(redact_bytes(content, redactor))
        _atomic_json(run_dir / "outcome.json", outcome, redactor)
    except CampaignError:
        keys = ("run_id", "task_id", "arm", "repeat", "order", "manifest_sha256", "fixture_commit", "model", "effort", "harness", "engine_sha256", "extractor_sha256")
        outcome = {key: outcome[key] for key in keys if key in outcome}
        outcome.update({"status": "retention_redaction_failed", "human_review_required": True})
        (run_dir / "outcome.json").write_text(json.dumps(outcome, sort_keys=True) + "\n")
    (run_dir / "in_progress.json").unlink(missing_ok=True)
    return outcome


def _trial_in_root(args, job, task, experiment, manifest_hash, seed, commit, auth_file, hidden_roots, capture_fn, redactor, run_root):
    run_root = Path(run_root)
    project = run_root / "project"
    shutil.copytree(seed, project)
    io = run_root / "io"
    io.mkdir()
    schema_source = Path(args.root) / REVIEW / "answer-schema.json"
    schema = io / "answer-schema.json"
    schema.write_bytes(schema_source.read_bytes())
    answer = io / "answer.json"
    telemetry = io / "telemetry"
    telemetry.mkdir()
    guidance_prompt, guidance, schema_bytes, prompt_redacted = outbound_payload(args.root, task, job["arm"], redactor)
    (io / "prompt.txt").write_text(guidance_prompt)
    (io / "guidance.md").write_text(guidance)
    cold_index_seconds = None
    preindex_result = None
    preindex_artifacts = {}
    binary = None
    if job["arm"] != "native":
        tools_dir = run_root / "tools"
        tools_dir.mkdir()
        binary = tools_dir / "code-kb"
        julie = tools_dir / "julie-extract"
        shutil.copy2(args.binary, binary)
        shutil.copy2(args.julie, julie)
        preindex = capture_fn(
            ["env", f"JULIE_EXTRACT_BIN={julie}", str(binary), "scan", "--root", str(project)],
            "", run_root / "preindex", timeout=360,
        )
        preindex_result = preindex
        cold_index_seconds = preindex["wall_seconds"]
        for name, suffix in (("events.jsonl", "stdout.log"), ("stderr.log", "stderr.log")):
            path = run_root / "preindex" / name
            if path.is_file():
                preindex_artifacts[f"preindex/{suffix}"] = path.read_bytes()
        if preindex["timed_out"] or preindex["exit_code"] != 0:
            return {
                "status": "preindex_failed", "cold_index_seconds": cold_index_seconds,
                "preindex_exit_code": preindex["exit_code"], "preindex_timed_out": preindex["timed_out"],
                "preindex_wall_seconds": cold_index_seconds,
            }, preindex_artifacts
    private_profile = run_root / "private-profile"
    private_profile.mkdir()
    (private_profile / "worktrees").mkdir()
    profile_target = PROFILE_TARGET
    schema.write_bytes(schema_bytes)
    command = codex_command(
        args.codex, project, schema, answer, profile_target, guidance,
        binary=binary, telemetry=telemetry if binary else None,
    )
    isolated = isolate(command, run_root, private_profile, profile_target, auth_file, hidden_roots)
    _, current_manifest, _, _ = validate_manifest(args.root, args.binary, args.julie)
    if current_manifest != manifest_hash or (binary and (sha256(binary) != experiment["engine_sha256"] or sha256(julie) != experiment["extractor_sha256"])):
        raise CampaignError("manifest or copied tool changed before dispatch")
    allowed, policy_note = provider_policy(args.root)
    if not allowed:
        raise CampaignError("policy_denied")
    capture_dir = run_root / "capture"
    execution = capture_fn(isolated, guidance_prompt, capture_dir, timeout=360)
    trace_path = capture_dir / "events.jsonl"
    events, parse_errors = _read_trace(trace_path)
    metrics = summarize_trace(events)
    telemetry_rows, telemetry_error = _read_telemetry(telemetry)
    latency = _latency_summary(telemetry_rows or [])
    metrics.update({
        "query_latency_samples": latency["samples"],
        "query_latency_p50_ms": latency["p50_ms"],
        "query_latency_p95_ms": latency["p95_ms"],
    })
    metrics["events_available"] = trace_path.is_file() and trace_path.stat().st_size > 0
    if not metrics["events_available"]:
        for key in ("tool_calls", "code_kb_calls", "commands", "mcp_calls", "actions"):
            metrics[key] = None
    metrics["malformed_jsonl_lines"] = parse_errors
    answer_value, answer_bytes = None, None
    if answer.is_file():
        answer_bytes = answer.read_bytes()
        try:
            answer_value = json.loads(answer_bytes)
        except json.JSONDecodeError:
            pass
    grade = grade_answer(answer_value, experiment_oracle(args.root, task["id"])) if answer_value is not None and metrics["completed"] else None
    if execution["timed_out"]:
        status = "timed_out"
    elif execution["exit_code"] != 0 and not metrics["completed"]:
        status = "infrastructure_error"
    elif not metrics["completed"]:
        status = "incomplete_trace"
    elif answer_value is None:
        status = "invalid_answer"
    else:
        status = "answer_received"
    artifacts = {
        **preindex_artifacts,
        "prompt.txt": guidance_prompt.encode(),
        "guidance.md": guidance.encode(),
        "answer-schema.json": schema_bytes,
        "events.jsonl": (capture_dir / "events.jsonl").read_bytes() if (capture_dir / "events.jsonl").is_file() else b"",
        "stderr.log": (capture_dir / "stderr.log").read_bytes() if (capture_dir / "stderr.log").is_file() else b"",
    }
    if telemetry_rows is not None:
        artifacts["telemetry/rows.json"] = (json.dumps(telemetry_rows, sort_keys=True) + "\n").encode()
    if answer_bytes is not None:
        artifacts["answer.json" if answer_value is not None else "answer.txt"] = answer_bytes
    telemetry_files = []
    if telemetry_rows is not None:
        telemetry_files.append("telemetry/rows.json")
    if binary:
        for item in sorted(telemetry.rglob("*")):
            if item.is_file() and item.suffix in (".json", ".jsonl", ".log", ".txt"):
                relative = Path("telemetry") / item.relative_to(telemetry)
                artifacts[relative.as_posix()] = item.read_bytes()
                telemetry_files.append(relative.as_posix())
    outcome = {
        "run_id": run_identifier(job), "task_id": task["id"], "arm": job["arm"], "repeat": job["repeat"],
        "order": job["order"], "status": status, "exit_code": execution["exit_code"],
        "timed_out": execution["timed_out"], "wall_seconds": execution["wall_seconds"],
        "cold_index_seconds": cold_index_seconds,
        "preindex_exit_code": preindex_result["exit_code"] if preindex_result else None,
        "preindex_timed_out": preindex_result["timed_out"] if preindex_result else False,
        "preindex_wall_seconds": preindex_result["wall_seconds"] if preindex_result else None,
        "fixture_commit": commit,
        "model": experiment["model"], "effort": experiment["effort"], "harness": experiment["harness"],
        "engine_sha256": experiment["engine_sha256"], "extractor_sha256": experiment["extractor_sha256"],
        "manifest_sha256": manifest_hash,
        "guidance_sha256": experiment["files"][f"{REVIEW}/guidance/{job['arm']}.md"],
        "answer_schema_sha256": experiment["files"][f"{REVIEW}/answer-schema.json"],
        "tasks_sha256": experiment["files"][f"{REVIEW}/tasks.json"],
        "oracles_sha256": experiment["files"][f"{REVIEW}/oracles.json"],
        "julie_version": experiment.get("julie_version"), "metrics": metrics,
        "mechanical_grade": grade, "human_review_required": True, "billed_cost": None,
        "telemetry_files": telemetry_files, "policy_note": policy_note, "prompt_redacted": prompt_redacted,
    }
    if telemetry_error is not None:
        outcome["telemetry_error"] = telemetry_error
    return outcome, artifacts


def _trial(args, job, task, experiment, manifest_hash, seed, commit, auth_file, hidden_roots, capture_fn, redactor):
    with tempfile.TemporaryDirectory(prefix="code-kb-comparison-") as temporary:
        return _trial_in_root(
            args, job, task, experiment, manifest_hash, seed, commit, auth_file, hidden_roots,
            capture_fn, redactor, Path(temporary),
        )


def experiment_oracle(root, task_id):
    return json.loads((Path(root) / REVIEW / "oracles.json").read_text())["tasks"][task_id]


def run_identifier(job):
    return f"{job['task_id']}-{job['arm']}-r{job['repeat']}"


def _csv_row(outcome):
    metrics = outcome.get("metrics", {})
    grade = outcome.get("mechanical_grade") or {}
    values = {
        "run_id": outcome["run_id"], "task_id": outcome["task_id"], "fixture_commit": outcome.get("fixture_commit"),
        "arm": outcome["arm"], "repeat": outcome["repeat"], "order": outcome["order"],
        "model": outcome.get("model"), "effort": outcome.get("effort"), "harness": outcome.get("harness"),
        "guidance_revision": outcome["arm"], "memory_config": "disabled",
        "code_kb_version": outcome.get("engine_sha256", "")[:12], "julie_version": outcome.get("julie_version"),
        "tool_calls": metrics.get("tool_calls"), "code_kb_calls": metrics.get("code_kb_calls"),
        "input_tokens": metrics.get("input_tokens"), "output_tokens": metrics.get("output_tokens"),
        "cache_tokens": metrics.get("cache_tokens"), "wall_seconds": outcome.get("wall_seconds"),
        "cold_index_seconds": outcome.get("cold_index_seconds"), "retries": 0,
        "query_p50_ms": metrics.get("query_latency_p50_ms"), "query_p95_ms": metrics.get("query_latency_p95_ms"),
        "acceptance": "pending_source_review" if grade else outcome.get("status"),
        "metric_availability": ";".join(f"{key}={'available' if metrics.get(key) is not None else 'unavailable'}" for key in ("input_tokens", "output_tokens", "cache_tokens", "tool_calls", "code_kb_calls")) + ";billing=unavailable;query_latency=" + ("available" if metrics.get("query_latency_samples", 0) else "unavailable"),
        "notes": f"{outcome.get('policy_note', '')}; status={outcome.get('status')}; mechanical_pass={grade.get('mechanical_pass', 'unavailable')}; human source review required",
    }
    return {key: "" if value is None else value for key, value in values.items()}


def _write_csv(output):
    output = Path(output)
    template = output.parent.parent / REVIEW / "run-log-template.csv"
    if not template.is_file():
        template = Path(__file__).resolve().parents[1] / REVIEW / "run-log-template.csv"
    with template.open(newline="") as stream:
        fields = next(csv.reader(stream))
    rows = []
    for path in (output / "runs").glob("*/outcome.json"):
        try:
            rows.append(json.loads(path.read_text()))
        except json.JSONDecodeError:
            continue
    rows.sort(key=lambda row: (row.get("task_id", ""), row.get("repeat", 0), row.get("order", 0)))
    temporary = output / "results.csv.tmp"
    with temporary.open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields)
        writer.writeheader()
        writer.writerows(_csv_row(row) for row in rows)
    temporary.replace(output / "results.csv")


def run_campaign(args, *, jobs=None, auth_file=AUTH_FILE, hidden_roots=None, capture_fn=None, redactor=None):
    root, output = Path(args.root).resolve(), Path(args.output).resolve()
    if not args.run:
        return {"status": "not_run"}
    if not Path(auth_file).is_file():
        raise CampaignError("saved Codex authentication is unavailable")
    experiment, manifest_hash, task_data, _ = validate_manifest(root, args.binary, args.julie)
    task_map = {task["id"]: task for task in task_data["tasks"]}
    generated_schedule = jobs is None
    jobs = schedule(list(task_map)) if generated_schedule else jobs
    if generated_schedule and len(jobs) != 72:
        raise CampaignError("generated schedule does not match the frozen 72-trial design")
    seed, commit = prepare_seed(root, output)
    capture_fn = capture if capture_fn is None else capture_fn
    hidden_roots = [str(path) for path in HIDDEN_ROOTS if path.exists()] if hidden_roots is None else hidden_roots
    failures = {"preindex_failed", "infrastructure_error", "incomplete_trace", "preflight_failed", "policy_denied", "redaction_failed", "interrupted_unknown", "retention_redaction_failed"}
    prior_failures = []
    for position, job in enumerate(jobs, 1):
        run_id = run_identifier(job)
        run_dir = output / "runs" / run_id
        outcome_path, progress_path = run_dir / "outcome.json", run_dir / "in_progress.json"
        run_dir.mkdir(parents=True, exist_ok=True)
        if outcome_path.exists():
            try:
                previous = json.loads(outcome_path.read_text())
            except json.JSONDecodeError as error:
                raise CampaignError(f"saved outcome is invalid: {run_id}") from error
            if not isinstance(previous, dict) or previous.get("manifest_sha256") != manifest_hash:
                raise CampaignError(f"saved outcome manifest fingerprint mismatch: {run_id}")
            if previous.get("fixture_commit") != commit:
                raise CampaignError(f"saved outcome fixture fingerprint mismatch: {run_id}")
            if any(previous.get(key) != job.get(key) for key in ("task_id", "arm", "repeat", "order")):
                raise CampaignError(f"saved outcome run identity mismatch: {run_id}")
            if previous.get("status") in failures:
                prior_failures.append(run_id)
            print(f"[{position}/{len(jobs)}] {run_id}: saved {previous.get('status', 'unknown')}", file=sys.stderr, flush=True)
            continue
        if progress_path.exists():
            try:
                saved_progress = json.loads(progress_path.read_text())
            except json.JSONDecodeError as error:
                raise CampaignError(f"saved in-progress record is invalid: {run_id}") from error
            if not isinstance(saved_progress, dict) or saved_progress.get("manifest_sha256") != manifest_hash:
                raise CampaignError(f"in-progress manifest fingerprint mismatch: {run_id}")
            if saved_progress.get("fixture_commit") != commit:
                raise CampaignError(f"in-progress fixture fingerprint mismatch: {run_id}")
            if saved_progress.get("run_id") != run_id or any(
                saved_progress.get(key) != job.get(key) for key in ("task_id", "arm", "repeat", "order")
            ):
                raise CampaignError(f"in-progress run identity mismatch: {run_id}")
            required_metadata = ("model", "effort", "harness", "engine_sha256", "extractor_sha256", "julie_version")
            if any(key not in saved_progress for key in required_metadata):
                raise CampaignError(f"in-progress metadata is incomplete: {run_id}")
            outcome = dict(saved_progress)
            outcome.update({"status": "interrupted_unknown", "human_review_required": True})
            _atomic_json(run_dir / "outcome.json", outcome, redactor)
            _write_csv(output)
            print(f"[{position}/{len(jobs)}] {run_id}: interrupted_unknown", file=sys.stderr, flush=True)
            return outcome
        _atomic_json(progress_path, {
            "run_id": run_id, "task_id": job["task_id"], "arm": job["arm"],
            "repeat": job["repeat"], "order": job["order"],
            "manifest_sha256": manifest_hash, "fixture_commit": commit,
            "model": experiment["model"], "effort": experiment["effort"],
            "harness": experiment["harness"], "engine_sha256": experiment["engine_sha256"],
            "extractor_sha256": experiment["extractor_sha256"],
            "julie_version": experiment.get("julie_version"),
            "infrastructure_retries_used": 0,
            "dispatch_may_occur": True,
        }, redactor)
        attempt_dir = run_dir / "attempt-001"
        print(f"[{position}/{len(jobs)}] {run_id}: dispatching", file=sys.stderr, flush=True)
        try:
            experiment, manifest_hash, task_data, _ = validate_manifest(root, args.binary, args.julie)
            outcome, artifacts = _trial(args, job, task_map[job["task_id"]], experiment, manifest_hash, seed, commit, auth_file, hidden_roots, capture_fn, redactor)
            outcome["status"] = outcome.get("status", "preflight_failed")
        except ValueError as error:
            outcome, artifacts = ({"run_id": run_id, "task_id": job["task_id"], "arm": job["arm"], "repeat": job["repeat"], "order": job["order"], "status": "preflight_failed", "failure": str(error), "human_review_required": True}, {})
        except CampaignError as error:
            reason = str(error)
            status = "policy_denied" if reason == "policy_denied" else ("redaction_failed" if "redaction" in reason else "preflight_failed")
            outcome, artifacts = ({"run_id": run_id, "task_id": job["task_id"], "arm": job["arm"], "repeat": job["repeat"], "order": job["order"], "status": status, "human_review_required": True}, {})
            outcome["failure"] = status
        except OSError:
            outcome, artifacts = ({"run_id": run_id, "task_id": job["task_id"], "arm": job["arm"], "repeat": job["repeat"], "order": job["order"], "status": "infrastructure_error", "failure": "process startup failed", "human_review_required": True}, {})
        outcome.setdefault("run_id", run_id)
        outcome.setdefault("task_id", job["task_id"])
        outcome.setdefault("arm", job["arm"])
        outcome.setdefault("repeat", job["repeat"])
        outcome.setdefault("order", job["order"])
        outcome.setdefault("manifest_sha256", manifest_hash)
        outcome.setdefault("fixture_commit", commit)
        outcome.setdefault("model", experiment["model"])
        outcome.setdefault("effort", experiment["effort"])
        outcome.setdefault("harness", experiment["harness"])
        outcome.setdefault("engine_sha256", experiment["engine_sha256"])
        outcome.setdefault("extractor_sha256", experiment["extractor_sha256"])
        outcome.setdefault("julie_version", experiment.get("julie_version"))
        outcome.setdefault("infrastructure_retries_used", 0)
        outcome.setdefault("human_review_required", True)
        recorded = _record_outcome(run_dir, attempt_dir, outcome, artifacts, redactor)
        _write_csv(output)
        print(f"[{position}/{len(jobs)}] {run_id}: {recorded.get('status', 'unknown')}", file=sys.stderr, flush=True)
        if recorded.get("status") in failures:
            return recorded
    if prior_failures:
        _write_csv(output)
        return {"status": "incomplete", "runs": len(jobs), "infrastructure_outcomes": prior_failures, "seed_commit": commit}
    _write_csv(output)
    return {"status": "complete", "runs": len(jobs), "seed_commit": commit}


def main(argv=None):
    args = parse_args(argv)
    if not args.run:
        print("No dispatch. Pass --run to start the campaign.")
        return 0
    try:
        result = run_campaign(args)
    except (CampaignError, ValueError) as error:
        raise SystemExit(str(error)) from error
    if result.get("status") != "complete":
        print(json.dumps(result, sort_keys=True), file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
