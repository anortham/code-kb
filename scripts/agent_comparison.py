"""Run isolated, bounded agent retrieval comparisons."""

from pathlib import Path
import shutil
import os
import signal
import subprocess
import time
import json


def codex_command(codex, project, schema, answer, profile, guidance, binary=None, telemetry=None):
    values = {
        "approval_policy": "never",
        "model_reasoning_effort": "high",
        "project_doc_max_bytes": 0,
        "developer_instructions": guidance,
        "web_search": "disabled",
        "default_permissions": "comparison",
        "permissions.comparison.extends": ":read-only",
        "permissions.comparison.network.enabled": False,
        "memories.use_memories": False,
        "memories.generate_memories": False,
    }
    for feature in ("apps", "plugins", "hooks", "memories", "multi_agent", "shell_snapshot", "unbounded_connection_retries"):
        values[f"features.{feature}"] = False
    values["features.skip_host_skill_discovery"] = True
    if binary is not None:
        for tool in ("file_skeleton", "lookup_symbol", "get_symbol_body", "get_symbol_context", "find_references", "blast_radius", "check_edit"):
            values[f"mcp_servers.code_kb.tools.{tool}.approval_mode"] = "approve"
        values.update({
            "mcp_servers.code_kb.command": str(binary),
            "mcp_servers.code_kb.args": ["serve", "--root", str(project)],
            "mcp_servers.code_kb.env.CODE_KB_TELEMETRY_DIR": str(telemetry),
            "mcp_servers.code_kb.env.JULIE_EXTRACT_BIN": str(Path(binary).parent / "julie-extract"),
        })
    command = [str(codex), "--no-daemon", "exec", "--ignore-user-config", "--ignore-rules", "--ephemeral", "--strict-config", "--json", "--color", "never", "-m", "gpt-6.1-sol", "-C", str(project), "--output-schema", str(schema), "-o", str(answer)]
    for key, value in values.items():
        command += ["-c", f"{key}={json.dumps(value)}"]
    command += ["-c", "permissions.comparison.filesystem={" + json.dumps(str(profile)) + ' = "deny"}']
    return command + ["-"]


def capture(argv, prompt, output, timeout=360):
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    timed_out = False
    environment = {key: value for key, value in os.environ.items() if key not in {"CODEX_THREAD_ID", "CODEX_SESSION_ID", "CODEX_TOOL_CALL_ID"}}
    with (output / "events.jsonl").open("wb") as stdout, (output / "stderr.log").open("wb") as stderr:
        child = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=stdout, stderr=stderr, start_new_session=True, env=environment)
        try:
            child.communicate(prompt.encode(), timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
    return {"exit_code": child.returncode, "timed_out": timed_out, "wall_seconds": time.monotonic() - started}


def stage_fixture(source, destination):
    source, destination = Path(source), Path(destination)
    excluded = {"target", ".code-kb", "__pycache__", ".git"}
    forbidden = {"oracles.json", "tasks.json", ".memories", "AGENTS.md", "CLAUDE.md"}
    files = []
    for item in source.rglob("*"):
        relative = item.relative_to(source)
        if any(part in excluded for part in relative.parts):
            continue
        if item.is_symlink() or any(part in forbidden for part in relative.parts):
            raise ValueError(f"not a fixture-only input: {relative}")
        if item.is_file():
            files.append((item, relative))
    destination.mkdir()
    for item, relative in files:
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(item, target)


def isolate(argv, run_root, private_profile, profile_target, auth_file, hidden_roots):
    command = [
        "bwrap", "--die-with-parent", "--unshare-pid", "--unshare-ipc",
        "--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev",
        "--tmpfs", "/tmp", "--bind", str(run_root), str(run_root),
        "--bind", str(private_profile), str(profile_target),
        "--ro-bind", str(auth_file), str(Path(profile_target) / "auth.json"),
        "--chdir", str(Path(run_root) / "project"),
    ]
    for root in hidden_roots:
        command += ["--tmpfs", str(root)]
    return command + ["--", *argv]


def schedule(task_ids):
    orders = (
        ("native", "current", "revised"),
        ("revised", "native", "current"),
        ("current", "revised", "native"),
    )
    return [
        {"task_id": task, "repeat": repeat, "order": position, "arm": arm}
        for task in task_ids
        for repeat, arms in enumerate(orders, 1)
        for position, arm in enumerate(arms, 1)
    ]
