import json
import re
import shlex
from typing import Any


_UNAVAILABLE_COMMAND = re.compile(
    r"\bno runnable\b.*\btest command\b|\btest command is unavailable\b",
    re.IGNORECASE,
)
_RUN_COMMAND = re.compile(r"\bRun\s+`([^`]+)`", re.IGNORECASE)
_USAGE_FIELDS = {
    "input_tokens": ("input_tokens", "prompt_tokens"),
    "output_tokens": ("output_tokens", "completion_tokens"),
    "cache_tokens": ("cached_input_tokens", "cache_tokens", "cached_tokens"),
}
_COMMAND_TYPES = {"commandexecution", "command_execution", "exec", "shell"}
_MCP_TYPES = {"mcptoolcall", "mcp_tool_call"}
_GENERIC_CALL_TYPES = {"function_call", "tool_call", "custom_tool_call"}
_RUST_FIXTURE_MANIFESTS = {
    "evals/review-2026-10-26/fixture/Cargo.toml",
    "Cargo.toml",
}


def _kind(event: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    payload = event.get("payload")
    if not isinstance(payload, dict):
        payload = {}
    if event.get("type") == "event_msg":
        return str(payload.get("type", "")), payload
    return str(event.get("type", "")), event


def _usage_values(usage: Any) -> dict[str, int | None] | None:
    if not isinstance(usage, dict):
        return None
    total = usage.get("total_token_usage")
    if isinstance(total, dict):
        usage = total
    values: dict[str, int | None] = {}
    for target, source_names in _USAGE_FIELDS.items():
        value = next((usage[name] for name in source_names if name in usage), None)
        values[target] = value if isinstance(value, int) and not isinstance(value, bool) else None
    return values


def _event_usage(kind: str, payload: dict[str, Any]) -> tuple[str, dict[str, int | None]] | None:
    info = payload.get("info")
    if kind == "token_count" and isinstance(info, dict):
        total = _usage_values(info.get("total_token_usage"))
        if total is not None:
            return "total", total
    usage = _usage_values(payload.get("usage"))
    if usage is not None:
        return "turn", usage
    return None


def _item_for(event: dict[str, Any], kind: str, payload: dict[str, Any]) -> dict[str, Any] | None:
    if kind in {"item.started", "item_started", "item.completed", "item_completed"}:
        item = payload.get("item")
        return item if isinstance(item, dict) else None
    return None


def _item_identity(item: dict[str, Any]) -> str | None:
    for field in ("id", "item_id", "call_id"):
        value = item.get(field)
        if value is not None:
            return str(value)
    return None


def _arguments(item: dict[str, Any]) -> Any:
    value = item.get("arguments", item.get("input", item.get("args")))
    if isinstance(value, str):
        try:
            decoded = json.loads(value)
        except json.JSONDecodeError:
            return value
        return decoded
    return value


def _tool_name(item: dict[str, Any]) -> str:
    value = item.get("tool") or item.get("tool_name") or item.get("name") or ""
    return str(value)


def _server_name(item: dict[str, Any]) -> str | None:
    value = item.get("server") or item.get("server_name")
    return str(value) if value is not None else None


def _mcp_name(item: dict[str, Any], tool: str) -> tuple[str | None, str]:
    server = _server_name(item)
    if server is not None:
        return server, tool
    match = re.fullmatch(r"mcp__([^_]+(?:_[^_]+)*)__([^_]+(?:_[^_]+)*)", tool)
    if match:
        return match.group(1), match.group(2)
    if tool.startswith("mcp__"):
        parts = tool.split("__", 2)
        if len(parts) == 3:
            return parts[1], parts[2]
    if "." in tool and tool.split(".", 1)[0].lower() in {"code-kb", "code_kb"}:
        return tool.split(".", 1)[0], tool.split(".", 1)[1]
    return server, tool


def _is_code_kb(server: str | None, tool: str) -> bool:
    server_name = (server or "").lower().replace("_", "-")
    tool_name = tool.lower().replace("_", "-")
    return server_name in {"code-kb", "codekb"} or tool_name.startswith("mcp--code-kb--") or tool_name.startswith("code-kb.")


def _command_text(item: dict[str, Any], tool: str) -> str | None:
    command = item.get("command")
    if isinstance(command, (list, tuple)):
        command = command[-1] if command else None
    if not isinstance(command, str) and tool in {"exec_command", "shell_command", "bash"}:
        arguments = _arguments(item)
        if isinstance(arguments, dict):
            command = arguments.get("cmd") or arguments.get("command")
    if isinstance(command, str) and command.strip():
        return command.strip()
    return None


def _completed_item(item: dict[str, Any]) -> tuple[bool, dict[str, Any] | None, str | None]:
    item_type = str(item.get("type", "")).lower().replace("-", "_")
    compact_type = item_type.replace("_", "")
    tool = _tool_name(item)
    server = _server_name(item)
    is_mcp = compact_type in _MCP_TYPES or server is not None or tool.startswith("mcp__")
    is_command = compact_type in _COMMAND_TYPES or tool in {"exec_command", "shell_command", "bash"}
    is_call = is_mcp or is_command or item_type in _GENERIC_CALL_TYPES or (
        item_type.endswith("_tool_call") and not item_type.endswith("_tool_call_output")
    )
    if not is_call:
        return False, None, None
    if is_mcp:
        normalized_server, normalized_tool = _mcp_name(item, tool)
        return True, {
            "id": _item_identity(item),
            "server": normalized_server,
            "tool": normalized_tool,
            "arguments": _arguments(item),
        }, None
    if is_command:
        return True, None, _command_text(item, tool)
    if item_type in _GENERIC_CALL_TYPES:
        return True, None, _command_text(item, tool)
    return True, None, None


def _event_error(kind: str, payload: dict[str, Any], item: dict[str, Any] | None) -> str | None:
    if kind in {"error", "turn.failed", "turn_failed", "item.failed", "item_failed", "task_failed"}:
        nested_item = payload.get("item")
        if not isinstance(nested_item, dict):
            nested_item = {}
        value = payload.get("error") or payload.get("message") or payload.get("detail") or nested_item.get("error")
        if isinstance(value, dict):
            value = value.get("message") or value.get("detail") or value
        if value is not None:
            return value if isinstance(value, str) else json.dumps(value, sort_keys=True)
        return kind
    if item is None:
        return None
    exit_code = item.get("exit_code")
    is_error = item.get("is_error") or item.get("error") or item.get("status") in ("failed", "error")
    if exit_code not in (None, 0) or is_error:
        value = item.get("aggregated_output") or item.get("error") or item.get("stderr")
        if isinstance(value, dict):
            value = value.get("message") or value.get("detail") or value
        if isinstance(value, str) and value.strip():
            return value.strip()
        return f"command exited with status {exit_code}" if exit_code not in (None, 0) else "tool call failed"
    return None


def summarize_trace(events: list[dict]) -> dict:
    """Summarize completed Codex JSONL items and token usage without guessing missing metrics."""
    completed_ids: set[str] = set()
    tool_calls = 0
    code_kb_calls = 0
    commands: list[str] = []
    mcp_calls: list[dict[str, Any]] = []
    first_seen: dict[str, int] = {}
    actions: list[tuple[int, dict[str, Any]]] = []
    errors: list[str] = []
    turn_usage: dict[str, dict[str, int | None]] = {}
    usage_snapshots: list[dict[str, int | None]] = []
    completed = False

    for index, event in enumerate(events):
        if not isinstance(event, dict):
            continue
        kind, payload = _kind(event)
        if kind in {"turn.completed", "thread.completed", "response.completed", "task_complete", "task_completed"}:
            completed = True
        usage = _event_usage(kind, payload)
        if usage is not None:
            usage_kind, values = usage
            if usage_kind == "total":
                usage_snapshots.append(values)
            else:
                turn_id = str(event.get("turn_id") or payload.get("turn_id") or index)
                turn_usage[turn_id] = values
        event_error = _event_error(kind, payload, None)
        if event_error is not None and event_error not in errors:
            errors.append(event_error)

        item = _item_for(event, kind, payload)
        item_id = _item_identity(item) if item is not None else None
        if item_id is not None:
            first_seen.setdefault(item_id, index)
        if kind in {"item.started", "item_started"}:
            continue
        if kind not in {"item.completed", "item_completed"}:
            continue
        if item is not None and item_id is not None and item_id in completed_ids:
            continue
        if item is not None and item_id is not None:
            completed_ids.add(item_id)
        counted, mcp_call, command = _completed_item(item) if item is not None else (False, None, None)
        if counted:
            tool_calls += 1
            if mcp_call is not None:
                mcp_calls.append(mcp_call)
                actions.append((first_seen.get(item_id, index), {"kind": "mcp", **mcp_call}))
                if _is_code_kb(mcp_call.get("server"), mcp_call.get("tool", "")):
                    code_kb_calls += 1
            if command is not None:
                commands.append(command)
            if command is not None or (item is not None and str(item.get("type", "")).lower().replace("-", "_") in _COMMAND_TYPES):
                actions.append((first_seen.get(item_id, index), {"id": item_id, "kind": "command", "command": command}))
            elif mcp_call is None and item is not None:
                actions.append((first_seen.get(item_id, index), {
                    "id": item_id,
                    "kind": "tool",
                    "tool": _tool_name(item) or item.get("type"),
                    "arguments": _arguments(item),
                }))
        error = _event_error(kind, payload, item)
        if error is not None and error not in errors:
            errors.append(error)

    if usage_snapshots:
        totals = usage_snapshots[-1]
    elif turn_usage:
        totals = {}
        for field in _USAGE_FIELDS:
            values = [usage[field] for usage in turn_usage.values()]
            totals[field] = sum(values) if values and all(value is not None for value in values) else None
    else:
        totals = {field: None for field in _USAGE_FIELDS}

    return {
        "input_tokens": totals["input_tokens"],
        "output_tokens": totals["output_tokens"],
        "cache_tokens": totals["cache_tokens"],
        "tool_calls": tool_calls,
        "code_kb_calls": code_kb_calls,
        "commands": commands,
        "mcp_calls": mcp_calls,
        "actions": [action for _, action in sorted(actions, key=lambda row: row[0])],
        "errors": errors,
        "completed": completed,
    }


def _path(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    path = value.replace("\\", "/")
    while path.startswith("./"):
        path = path[2:]
    return path.rstrip("/") or "."


def _valid_line(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _list_field(answer: dict[str, Any], key: str, failures: list[str]) -> list[Any]:
    value = answer.get(key)
    if isinstance(value, list):
        return value
    failures.append(f"answer field `{key}` must be a list")
    return []


def _test_parts(value: Any) -> tuple[str | None, str | None]:
    if not isinstance(value, str):
        return None, None
    if "::" in value:
        file, terminal = value.split("::", 1)
    else:
        file, terminal = None, value
    terminal = terminal.rsplit("::", 1)[-1].rsplit(".", 1)[-1]
    return _path(file) if file else None, terminal


def _answer_test_matches(row: dict[str, Any], expected_file: str, expected_name: str) -> bool:
    actual_file = _path(row.get("file"))
    _, actual_name = _test_parts(row.get("name"))
    return actual_file == expected_file and actual_name == expected_name


def _expected_command(oracle: dict[str, Any]) -> str | None:
    acceptance = oracle.get("acceptance", [])
    if not isinstance(acceptance, list):
        return None
    for statement in acceptance:
        if not isinstance(statement, str):
            continue
        match = _RUN_COMMAND.search(statement)
        if match:
            return match.group(1)
    return None


def _command_matches(actual: Any, expected: str, alternatives: Any = None) -> bool:
    if not isinstance(actual, str):
        return False
    try:
        actual_parts = shlex.split(actual)
    except ValueError:
        return False

    alternative_parts: list[list[str]] = []
    if isinstance(alternatives, list):
        try:
            if all(isinstance(command, str) and command.strip() for command in alternatives):
                alternative_parts = [shlex.split(command) for command in alternatives]
        except ValueError:
            alternative_parts = []
    if alternative_parts and all(alternative_parts) and actual_parts in alternative_parts:
        return True

    try:
        expected_parts = shlex.split(expected)
    except ValueError:
        return False
    if actual_parts == expected_parts:
        return True
    try:
        manifest_index = expected_parts.index("--manifest-path") + 1
    except (ValueError, IndexError):
        return False
    if manifest_index >= len(expected_parts):
        return False
    expected_manifest = expected_parts[manifest_index]
    actual_manifest_index = None
    for index, part in enumerate(actual_parts[:-1]):
        if part == "--manifest-path":
            actual_manifest_index = index + 1
            break
        if part.startswith("--manifest-path="):
            actual_manifest_index = index + 1
            actual_parts = actual_parts[:index] + ["--manifest-path", part.split("=", 1)[1]] + actual_parts[index + 1 :]
            break
    if actual_manifest_index is None or actual_manifest_index >= len(actual_parts):
        return False
    if expected_manifest not in _RUST_FIXTURE_MANIFESTS or actual_parts[actual_manifest_index] not in _RUST_FIXTURE_MANIFESTS:
        return False
    expected_parts[manifest_index] = actual_parts[actual_manifest_index]
    return actual_parts == expected_parts


def grade_answer(answer: dict, oracle: dict) -> dict:
    """Check answer facts against the oracle; source-chain claims always need human review."""
    failures: list[str] = []
    checks: list[str] = []
    if not isinstance(answer, dict):
        answer = {}
        failures.append("answer must be an object")
    if not isinstance(oracle, dict):
        oracle = {}
        failures.append("oracle must be an object")

    symbols = _list_field(answer, "symbols", failures)
    tests = _list_field(answer, "tests", failures)
    literals = _list_field(answer, "literals", failures)
    if not isinstance(answer.get("explanation"), str):
        failures.append("answer field `explanation` must be a string")

    valid_symbols: list[dict[str, Any]] = []
    for index, row in enumerate(symbols):
        if not isinstance(row, dict):
            failures.append(f"symbol entry {index + 1} must be an object")
            continue
        if not isinstance(row.get("name"), str) or _path(row.get("file")) is None or not _valid_line(row.get("line")):
            failures.append(f"symbol entry {index + 1} must include a name, file, and integer line")
            continue
        if "owner" not in row or (row.get("owner") is not None and not isinstance(row.get("owner"), str)):
            failures.append(f"symbol entry {index + 1} must include a null or string owner")
            continue
        if not isinstance(row.get("reason"), str):
            failures.append(f"symbol entry {index + 1} must include a reason")
            continue
        valid_symbols.append(row)

    valid_tests: list[dict[str, Any]] = []
    for index, row in enumerate(tests):
        if not isinstance(row, dict):
            failures.append(f"test entry {index + 1} must be an object")
            continue
        if not isinstance(row.get("name"), str) or _path(row.get("file")) is None:
            failures.append(f"test entry {index + 1} must include a name and file")
            continue
        if row.get("command") is not None and not isinstance(row.get("command"), str):
            failures.append(f"test entry {index + 1} command must be a string or null")
            continue
        if row.get("command_status") not in ("evidenced", "unavailable"):
            failures.append(f"test entry {index + 1} command_status must be `evidenced` or `unavailable`")
            continue
        if not isinstance(row.get("reason"), str):
            failures.append(f"test entry {index + 1} must include a reason")
            continue
        if (row.get("command") is None) != (row.get("command_status") == "unavailable"):
            failures.append(f"test entry {index + 1} command and command_status disagree")
            continue
        valid_tests.append(row)

    valid_literals: list[dict[str, Any]] = []
    for index, row in enumerate(literals):
        if not isinstance(row, dict):
            failures.append(f"literal entry {index + 1} must be an object")
            continue
        if not isinstance(row.get("value"), str) or _path(row.get("file")) is None or not _valid_line(row.get("line")):
            failures.append(f"literal entry {index + 1} must include a value, file, and integer line")
            continue
        valid_literals.append(row)

    for expected in oracle.get("symbols", []):
        if not isinstance(expected, dict):
            continue
        expected_name = expected.get("name")
        expected_file = _path(expected.get("file"))
        allowed_lines = {line for line in (expected.get("line"), expected.get("definition_line")) if _valid_line(line)}
        matches = [
            row
            for row in valid_symbols
            if row.get("name") == expected_name
            and _path(row.get("file")) == expected_file
            and row.get("line") in allowed_lines
            and ("owner" not in expected or row.get("owner") == expected.get("owner"))
        ]
        if matches:
            checks.append(f"symbol {expected_name} at {expected_file}:{'/'.join(map(str, sorted(allowed_lines)))} matched")
        else:
            locations = ", ".join(str(line) for line in sorted(allowed_lines)) or "the oracle line"
            failures.append(f"missing symbol {expected_name} in {expected_file} at {locations}")

    for excluded in oracle.get("excluded_candidates", []):
        if not isinstance(excluded, dict):
            continue
        excluded_name = excluded.get("name")
        excluded_file = _path(excluded.get("file"))
        excluded_line = excluded.get("line")
        excluded_owner = excluded.get("owner")
        for row in valid_symbols:
            same_name_file = row.get("name") == excluded_name and _path(row.get("file")) == excluded_file
            exact_candidate = same_name_file and row.get("line") == excluded_line
            conflicting_owner = same_name_file and excluded_owner is not None and row.get("owner") == excluded_owner
            if exact_candidate or conflicting_owner:
                failures.append(f"excluded candidate {excluded_name} owned by {excluded_owner} was reported")
                break

    for expected in oracle.get("symbols", []):
        if not isinstance(expected, dict):
            continue
        name, file = expected.get("name"), _path(expected.get("file"))
        owners = {
            row["owner"]
            for row in valid_symbols
            if row.get("name") == name and _path(row.get("file")) == file and row.get("owner") is not None
        }
        if len(owners) > 1:
            failures.append(f"symbol {name} in {file} has conflicting owners: {', '.join(sorted(owners))}")

    expected_tests: list[tuple[str, str]] = []
    for expected in oracle.get("tests", []):
        expected_file, expected_name = _test_parts(expected)
        if expected_file is None or expected_name is None:
            continue
        expected_tests.append((expected_file, expected_name))
        if any(_answer_test_matches(row, expected_file, expected_name) for row in valid_tests):
            checks.append(f"test {expected_name} in {expected_file} matched")
        else:
            failures.append(f"missing focused test {expected_name} in {expected_file}")

    for row in valid_tests:
        if not any(_answer_test_matches(row, file, name) for file, name in expected_tests):
            failures.append(f"unexpected test {row['name']} in {row['file']}")

    for expected in oracle.get("literals", []):
        if not isinstance(expected, dict):
            continue
        matches = [
            row
            for row in valid_literals
            if row.get("value") == expected.get("value")
            and _path(row.get("file")) == _path(expected.get("file"))
            and row.get("line") == expected.get("line")
        ]
        if matches:
            checks.append(f"literal {expected.get('value')!r} at {_path(expected.get('file'))}:{expected.get('line')} matched")
        else:
            failures.append(
                f"missing literal {expected.get('value')!r} in {_path(expected.get('file'))} at line {expected.get('line')}"
            )

    expected_command = _expected_command(oracle)
    if expected_command is not None:
        matching_command = any(
            any(_answer_test_matches(row, file, name) for file, name in expected_tests)
            and row.get("command_status") == "evidenced"
            and _command_matches(row.get("command"), expected_command, oracle.get("command_alternatives"))
            for row in valid_tests
        )
        if matching_command:
            checks.append("focused command matches the required command or a declared alternative")
        else:
            failures.append("missing the focused command required by the oracle")
    elif any(
        isinstance(statement, str) and _UNAVAILABLE_COMMAND.search(statement)
        for statement in oracle.get("acceptance", [])
    ):
        unavailable = any(
            any(_answer_test_matches(row, file, name) for file, name in expected_tests)
            and row.get("command") is None
            and row.get("command_status") == "unavailable"
            for row in valid_tests
        )
        if unavailable:
            checks.append("focused command is explicitly unavailable as the oracle requires")
        else:
            failures.append("oracle requires an unavailable focused command, but the answer did not say so")

    checks.append("source-chain explanations and semantic claims require source review")
    return {
        "mechanical_pass": not failures,
        "checks": checks,
        "failures": failures,
        "review_required": True,
    }
