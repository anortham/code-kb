"""Run child commands in an allowlisted bubblewrap filesystem.

Python API: ``run(project, state, output, timeout, mode, argv, *, dependencies=None, tools=None, protect=(), network=False, private_profile=None, auth_file=None)``.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path, PurePosixPath
from typing import Any, Mapping, Sequence


CONTAINER_PROJECT = "/workspace/project"
CONTAINER_STATE = "/state"
CONTAINER_DEPS = "/deps"
CONTAINER_TOOLS = "/tools"
PROFILE_NAME = "edit_evaluation"
PROFILE_PARENT = ":workspace"
NAME_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*\Z")
RESERVED_ROOTS = (
    "/bin",
    "/dev",
    "/etc",
    "/lib",
    "/lib64",
    "/proc",
    "/run",
    "/sbin",
    "/state",
    "/sys",
    "/tmp",
    "/tools",
    "/usr",
    "/workspace",
    "/deps",
)


class SandboxError(ValueError):
    pass


def _absolute_path(value: str | Path, label: str, *, must_exist: bool) -> Path:
    path = Path(value).expanduser()
    if not path.is_absolute():
        raise SandboxError(f"{label} must be an absolute path: {path}")
    try:
        resolved = path.resolve(strict=must_exist)
    except OSError as error:
        raise SandboxError(f"{label} does not resolve: {path}: {error}") from error
    if must_exist and not resolved.exists():
        raise SandboxError(f"{label} does not exist: {path}")
    return resolved


def _inside(left: Path, right: Path) -> bool:
    return left == right or left in right.parents


def _under_codex_worktrees(path: Path, codex_home: Path) -> bool:
    worktrees = codex_home / "worktrees"
    return path != worktrees and _inside(worktrees, path)


def _ensure_disjoint(label: str, paths: Mapping[str, Path]) -> None:
    names = list(paths)
    for index, left_name in enumerate(names):
        left = paths[left_name]
        for right_name in names[index + 1 :]:
            right = paths[right_name]
            if _inside(left, right) or _inside(right, left):
                raise SandboxError(f"{label} paths overlap: {left_name}={left} and {right_name}={right}")


def _parse_named_paths(values: Mapping[str, str | Path] | None, label: str) -> dict[str, Path]:
    parsed: dict[str, Path] = {}
    for name, value in (values or {}).items():
        if not NAME_PATTERN.fullmatch(name):
            raise SandboxError(f"invalid {label} name: {name!r}")
        parsed[name] = _absolute_path(value, f"{label} {name}", must_exist=True)
    return parsed


def _parse_cli_named_paths(values: Sequence[str] | None, label: str) -> dict[str, Path]:
    parsed: dict[str, Path] = {}
    for value in values or ():
        if "=" not in value:
            raise SandboxError(f"{label} must be name=absolute-path: {value!r}")
        name, path = value.split("=", 1)
        if not NAME_PATTERN.fullmatch(name) or not path:
            raise SandboxError(f"invalid {label}: {value!r}")
        if name in parsed:
            raise SandboxError(f"duplicate {label} name: {name}")
        parsed[name] = _absolute_path(path, f"{label} {name}", must_exist=True)
    return parsed


def _protected_paths(project: Path, values: Sequence[str]) -> list[tuple[str, Path]]:
    protected: list[tuple[str, Path]] = []
    seen: set[str] = set()
    for value in values:
        relative = PurePosixPath(value)
        if relative.is_absolute() or not relative.parts or any(part in (".", "..") for part in value.split("/")):
            raise SandboxError(f"protected path must be a non-empty relative path without traversal: {value!r}")
        if "\\" in value:
            raise SandboxError(f"protected path must use forward slashes: {value!r}")
        normalized = relative.as_posix()
        if normalized in seen:
            continue
        current = project
        for part in relative.parts:
            current = current / part
            if current.is_symlink():
                raise SandboxError(f"protected path traverses a symlink: {value!r}")
        if not current.exists():
            raise SandboxError(f"protected path does not exist in project: {value!r}")
        seen.add(normalized)
        protected.append((normalized, current))
    protected.sort(key=lambda item: (len(PurePosixPath(item[0]).parts), item[0]))
    result: list[tuple[str, Path]] = []
    for relative, source in protected:
        if any(_inside(PurePosixPath(parent), PurePosixPath(relative)) for parent, _ in result):
            continue
        result.append((relative, source))
    return result


def _protected_directory_mounts(project: Path, protected: Sequence[tuple[str, Path]]) -> list[tuple[str, Path]]:
    directories: dict[str, Path] = {}
    for relative, _ in protected:
        parent = PurePosixPath(relative).parent
        while parent.parts:
            normalized = parent.as_posix()
            directories[normalized] = project.joinpath(*parent.parts)
            parent = parent.parent
    return sorted(directories.items(), key=lambda item: (len(PurePosixPath(item[0]).parts), item[0]))


def _identity_paths() -> tuple[str | None, str | None, Path | None, Path | None]:
    home_value = os.environ.get("HOME")
    codex_value = os.environ.get("CODEX_HOME")
    home = Path(home_value) if home_value is not None else Path.home()
    if not home.is_absolute() or ".." in home.parts or home == Path("/"):
        raise SandboxError(f"HOME must be a non-root absolute path: {str(home)!r}")
    if codex_value is not None:
        codex_home = Path(codex_value)
    else:
        codex_home = home / ".codex"
    if codex_home is not None and (not codex_home.is_absolute() or ".." in codex_home.parts or codex_home == Path("/")):
        raise SandboxError(f"CODEX_HOME must be a non-root absolute path: {str(codex_home)!r}")
    for label, path in (("HOME", home), ("CODEX_HOME", codex_home)):
        if path is not None and any(_inside(Path(root), path) for root in RESERVED_ROOTS):
            raise SandboxError(f"{label} overlaps a reserved container path: {path}")
    if codex_home is not None and _inside(codex_home, home) and codex_home != home:
        raise SandboxError("CODEX_HOME must not contain HOME")
    return home_value, codex_value, home, codex_home


def _make_container_path(args: list[str], path: Path) -> None:
    current = Path("/")
    for part in path.parts[1:]:
        current /= part
        args.extend(("--dir", current.as_posix()))


def _system_mounts(args: list[str], network: bool) -> None:
    args.extend(("--dir", "/usr", "--ro-bind", "/usr", "/usr"))
    for path in ("/bin", "/sbin", "/lib", "/lib64"):
        host_path = Path(path)
        if not host_path.exists() and not host_path.is_symlink():
            continue
        if host_path.is_symlink():
            target = os.readlink(host_path)
            if target.startswith("/"):
                target = target[1:]
            args.extend(("--symlink", target, path))
        else:
            _make_container_path(args, Path(path))
            args.extend(("--ro-bind", path, path))
    args.extend(("--dir", "/etc"))
    for relative in ("passwd", "group", "nsswitch.conf", "localtime"):
        host_path = Path("/etc") / relative
        if host_path.exists():
            args.extend(("--ro-bind", str(host_path.resolve()), f"/etc/{relative}"))
    for host_path in (Path("/etc/ssl/certs"), Path("/etc/pki/ca-trust"), Path("/etc/pki/tls/certs")):
        if host_path.is_dir():
            destination = host_path.as_posix()
            _make_container_path(args, Path(destination))
            args.extend(("--ro-bind", str(host_path.resolve()), destination))
    if network:
        for relative in ("hosts", "resolv.conf"):
            host_path = Path("/etc") / relative
            if host_path.exists():
                args.extend(("--ro-bind", str(host_path.resolve()), f"/etc/{relative}"))


def _is_forbidden_codex_option(argument: str, forbidden: set[str]) -> bool:
    if argument.split("=", 1)[0] in forbidden:
        return True
    return any(argument.startswith(flag) and argument != flag for flag in ("-c", "-p", "-P", "-s", "-C"))


def _codex_arguments(argv: Sequence[str]) -> list[str]:
    if not argv:
        raise SandboxError("agent mode requires a Codex command after --")
    if argv[0] not in ("codex", "/tools/codex"):
        raise SandboxError("agent mode command must invoke the supplied codex tool")
    if len(argv) < 2 or argv[1] not in ("exec", "sandbox"):
        raise SandboxError("agent mode supports only codex exec or codex sandbox")
    forbidden = {
        "-c",
        "--config",
        "-p",
        "--profile",
        "-P",
        "--permission-profile",
        "-s",
        "--sandbox",
        "--add-dir",
        "--worktree",
        "--ignore-user-config",
        "--dangerously-bypass-approvals-and-sandbox",
        "--sandbox-state-json",
        "--sandbox-state-readable-root",
        "--include-managed-config",
    }
    if argv[1] == "sandbox":
        boundary = argv.index("--") if "--" in argv else len(argv)
        options = argv[2:boundary]
        controlled = any(_is_forbidden_codex_option(argument, forbidden) for argument in options)
        current_directory = None
        for index, argument in enumerate(options):
            if argument in ("-C", "--cd"):
                candidate = options[index + 1] if index + 1 < len(options) else None
                if candidate != CONTAINER_PROJECT:
                    controlled = True
                current_directory = candidate
            elif argument.startswith(("--cd=", "-C=")):
                candidate = argument.split("=", 1)[1]
                if candidate != CONTAINER_PROJECT:
                    controlled = True
                current_directory = candidate
        profile_options = ["--profile", PROFILE_NAME, "--permission-profile", PROFILE_NAME]
        if current_directory is None:
            profile_options.extend(("--cd", CONTAINER_PROJECT))
    else:
        controlled = False
        value_options = {"-m", "--model", "--local-provider", "--thread-source", "--color", "--output-schema", "-o", "--output-last-message", "-i", "--image"}
        index = 2
        current_directory = None
        while index < len(argv):
            argument = argv[index]
            if argument == "--":
                break
            if not argument.startswith("-"):
                index += 1
                continue
            if _is_forbidden_codex_option(argument, forbidden):
                controlled = True
                break
            if argument in ("-C", "--cd"):
                current_directory = argv[index + 1] if index + 1 < len(argv) else None
                if current_directory != CONTAINER_PROJECT:
                    controlled = True
                    break
                index += 2
            elif argument.startswith(("--cd=", "-C=")):
                current_directory = argument.split("=", 1)[1]
                if current_directory != CONTAINER_PROJECT:
                    controlled = True
                    break
                index += 1
            elif argument.startswith("-C"):
                controlled = True
                break
            else:
                index += 2 if argument in value_options and index + 1 < len(argv) else 1
        profile_options = ["--profile", PROFILE_NAME]
        if current_directory is None:
            profile_options.extend(("--cd", CONTAINER_PROJECT))
    if controlled:
        raise SandboxError("agent mode controls the Codex profile and sandbox flags")
    return [argv[0], argv[1], *profile_options, *argv[2:]]


def _codex_home_source(identity_root: Path, home: Path, codex_home: Path) -> Path:
    if _inside(home, codex_home):
        return identity_root / "home" / codex_home.relative_to(home)
    return identity_root / "codex-home"


def _profile_config(codex_home: Path) -> str:
    quoted_path = json.dumps(codex_home.as_posix())
    return (
        f'default_permissions = "{PROFILE_NAME}"\n\n'
        f"[permissions.{PROFILE_NAME}]\n"
        f"extends = {json.dumps(PROFILE_PARENT)}\n\n"
        f"[permissions.{PROFILE_NAME}.filesystem]\n"
        f"{quoted_path} = \"deny\"\n\n"
        '"/state" = "write"\n\n'
        f'[permissions.{PROFILE_NAME}.filesystem.":workspace_roots"]\n'
        f'"." = "write"\n\n'
        f"[permissions.{PROFILE_NAME}.network]\n"
        f"enabled = false\n"
    )


def _validate_private_profile(private_profile: Path | None) -> None:
    if private_profile is None:
        return
    base_config = private_profile / "config.toml"
    if not base_config.exists():
        return
    if base_config.is_symlink():
        raise SandboxError("private profile config must not be a symlink")
    try:
        parsed = tomllib.loads(base_config.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise SandboxError(f"private profile config is invalid: {error}") from error
    if "sandbox_mode" in parsed or "sandbox_workspace_write" in parsed:
        raise SandboxError("private profile config cannot use legacy sandbox settings")


def _populate_codex_home(private_profile: Path | None, destination: Path, codex_home: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True, mode=0o700)
    if private_profile is not None:
        for source_root, directories, filenames in os.walk(private_profile, followlinks=False):
            source_directory = Path(source_root)
            relative_directory = source_directory.relative_to(private_profile)
            target_directory = destination / relative_directory
            target_directory.mkdir(parents=True, exist_ok=True, mode=0o700)
            for name in directories:
                source = source_directory / name
                if source.is_symlink() or not source.is_dir():
                    raise SandboxError(f"private profile contains a non-directory or symlink: {source}")
                (target_directory / name).mkdir(exist_ok=True, mode=0o700)
            for name in filenames:
                source = source_directory / name
                if name == "auth.json":
                    raise SandboxError("private profile must not contain auth.json; provide it with --auth-file")
                if source.is_symlink() or not source.is_file():
                    raise SandboxError(f"private profile contains a non-regular file or symlink: {source}")
                if source == private_profile / f"{PROFILE_NAME}.config.toml":
                    continue
                target = target_directory / name
                shutil.copyfile(source, target)
                os.chmod(target, source.stat().st_mode & 0o777)
    generated_config = destination / f"{PROFILE_NAME}.config.toml"
    generated_config.write_text(_profile_config(codex_home), encoding="utf-8")
    generated_config.chmod(0o600)


def _build_bwrap_args(
    project: Path,
    state: Path,
    dependencies: Mapping[str, Path],
    tools: Mapping[str, Path],
    protected: Sequence[tuple[str, Path]],
    network: bool,
    agent_network: bool,
    home: Path | None,
    codex_home: Path | None,
    identity_root: Path,
    auth_file: Path | None,
) -> list[str]:
    executable = shutil.which("bwrap")
    if executable is None:
        raise SandboxError("bubblewrap (bwrap) is required")
    args = [executable, "--unshare-all", "--die-with-parent", "--new-session", "--cap-drop", "ALL"]
    if network or agent_network:
        args.append("--share-net")
    _system_mounts(args, network or agent_network)
    args.extend(("--proc", "/proc", "--dev", "/dev", "--dir", "/tmp", "--tmpfs", "/tmp", "--dir", "/run", "--tmpfs", "/run"))
    args.extend(("--dir", "/workspace", "--bind", str(project), CONTAINER_PROJECT))
    for relative, source in _protected_directory_mounts(project, protected):
        args.extend(("--bind", str(source), f"{CONTAINER_PROJECT}/{relative}"))
    args.extend(("--dir", CONTAINER_DEPS))
    for name, source in sorted(dependencies.items()):
        args.extend(("--ro-bind", str(source), f"{CONTAINER_DEPS}/{name}"))
    args.extend(("--dir", CONTAINER_TOOLS))
    for name, source in sorted(tools.items()):
        args.extend(("--ro-bind", str(source), f"{CONTAINER_TOOLS}/{name}"))
    args.extend(("--bind", str(state), CONTAINER_STATE))
    if home is not None:
        home_source = identity_root / "home"
        home_source.mkdir(parents=True, exist_ok=True)
        _make_container_path(args, home)
        args.extend(("--bind", str(home_source), home.as_posix()))
    if codex_home is not None:
        if home is not None and _inside(home, codex_home):
            codex_source = identity_root / "home" / codex_home.relative_to(home)
            codex_source.mkdir(parents=True, exist_ok=True)
        else:
            codex_source = identity_root / "codex-home"
            codex_source.mkdir(parents=True, exist_ok=True)
            _make_container_path(args, codex_home)
            args.extend(("--bind", str(codex_source), codex_home.as_posix()))
        if auth_file is not None:
            args.extend(("--ro-bind", str(auth_file), (codex_home / "auth.json").as_posix()))
    for relative, source in protected:
        args.extend(("--ro-bind", str(source), f"{CONTAINER_PROJECT}/{relative}"))
    args.append("--remount-ro")
    args.append("/")
    args.extend(("--chdir", CONTAINER_PROJECT))
    return args


def _terminate_process(process: subprocess.Popen[bytes], grace: float = 0.5) -> dict[str, Any]:
    attempted = False
    signal_name = None
    try:
        os.killpg(process.pid, signal.SIGTERM)
        attempted = True
        signal_name = "SIGTERM"
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
            attempted = True
            signal_name = "SIGKILL"
        except ProcessLookupError:
            pass
        process.wait()
    return {"attempted": attempted, "complete": process.poll() is not None, "signal": signal_name}


def _execute_sandbox(
    project: Path,
    state: Path,
    output: Path,
    timeout: float,
    mode: str,
    argv: Sequence[str],
    dependencies: Mapping[str, Path],
    tools: Mapping[str, Path],
    protected: Sequence[tuple[str, Path]],
    network: bool,
    private_profile: Path | None,
    auth_file: Path | None,
    home_value: str | None,
    codex_value: str | None,
    home: Path,
    codex_home: Path,
    identity_root: Path,
) -> dict[str, Any]:
    if mode == "agent":
        codex_source = _codex_home_source(identity_root, home, codex_home)
        _populate_codex_home(private_profile, codex_source, codex_home)
    stdout_log = output / "stdout.log"
    stderr_log = output / "stderr.log"
    bwrap_args = _build_bwrap_args(
        project,
        state,
        dependencies,
        tools,
        protected,
        network,
        mode == "agent",
        home,
        codex_home,
        identity_root,
        auth_file,
    )
    environment = {
        "PATH": "/tools:/usr/local/bin:/usr/bin:/bin",
        "PWD": CONTAINER_PROJECT,
        "TMPDIR": "/tmp",
        "TMP": "/tmp",
        "TEMP": "/tmp",
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TZ": "UTC",
        "XDG_CACHE_HOME": f"{CONTAINER_STATE}/cache",
        "XDG_CONFIG_HOME": f"{CONTAINER_STATE}/config",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
    }
    if home_value is not None:
        environment["HOME"] = home_value
    if codex_value is not None:
        environment["CODEX_HOME"] = codex_value
    for name, source in sorted(tools.items()):
        if source.is_dir():
            environment["PATH"] = f"{CONTAINER_TOOLS}/{name}/bin:{CONTAINER_TOOLS}/{name}:" + environment["PATH"]
    command = [*bwrap_args, "--clearenv"]
    for key, value in environment.items():
        command.extend(("--setenv", key, value))
    command.extend(("--", *argv))
    started = time.monotonic()
    timed_out = False
    with stdout_log.open("xb") as stdout_stream, stderr_log.open("xb") as stderr_stream:
        os.chmod(stdout_log, 0o600)
        os.chmod(stderr_log, 0o600)
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=stdout_stream, stderr=stderr_stream, start_new_session=True, close_fds=True)
        cleanup = {"attempted": False, "complete": True, "signal": None}
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            cleanup = _terminate_process(process)
    elapsed = time.monotonic() - started
    return {
        "mode": mode,
        "network_scope": "preparation" if network else "codex_provider_only" if mode == "agent" else "disabled",
        "exit_status": process.returncode,
        "timed_out": timed_out,
        "elapsed_seconds": round(elapsed, 6),
        "process_cleanup": cleanup,
        "stdout_log": str(stdout_log),
        "stderr_log": str(stderr_log),
    }


def run(
    project: str | Path,
    state: str | Path,
    output: str | Path,
    timeout: float,
    mode: str,
    argv: Sequence[str],
    *,
    dependencies: Mapping[str, str | Path] | None = None,
    tools: Mapping[str, str | Path] | None = None,
    protect: Sequence[str] = (),
    network: bool = False,
    private_profile: str | Path | None = None,
    auth_file: str | Path | None = None,
) -> dict[str, Any]:
    """Run one argv under bubblewrap and return exit, timeout, cleanup, and log details."""
    if mode not in ("prepare", "agent", "grade"):
        raise SandboxError(f"unsupported mode: {mode!r}")
    if not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or timeout <= 0:
        raise SandboxError("timeout must be a finite positive number of seconds")
    if network and mode != "prepare":
        raise SandboxError("--network is allowed only in preparation mode")
    if mode != "agent" and (private_profile is not None or auth_file is not None):
        raise SandboxError("private profile and auth file are allowed only in agent mode")
    project_path = _absolute_path(project, "project", must_exist=True)
    if not project_path.is_dir():
        raise SandboxError(f"project must be a directory: {project_path}")
    state_path = Path(state).expanduser()
    if not state_path.is_absolute():
        raise SandboxError(f"state must be an absolute path: {state_path}")
    if state_path.is_symlink():
        raise SandboxError(f"state must not be a symlink: {state_path}")
    state_path = state_path.resolve(strict=False)
    if state_path.exists() and not state_path.is_dir():
        raise SandboxError(f"state must be a directory: {state_path}")
    output_path = Path(output).expanduser()
    if not output_path.is_absolute():
        raise SandboxError(f"output must be an absolute path: {output_path}")
    if output_path.exists() or output_path.is_symlink():
        raise SandboxError(f"output directory already exists: {output_path}")
    output_path = output_path.resolve(strict=False)
    dep_paths = _parse_named_paths(dependencies, "dependency")
    tool_paths = _parse_named_paths(tools, "tool")
    protected = _protected_paths(project_path, protect)
    home_value, codex_value, home_path, codex_home_path = _identity_paths()
    private_profile_path = None
    if private_profile is not None:
        private_profile_path = _absolute_path(private_profile, "private profile", must_exist=True)
        if not private_profile_path.is_dir():
            raise SandboxError(f"private profile must be a directory: {private_profile_path}")
    _validate_private_profile(private_profile_path)
    auth_file_path = None
    if auth_file is not None:
        auth_file_path = _absolute_path(auth_file, "auth file", must_exist=True)
        if not auth_file_path.is_file():
            raise SandboxError(f"auth file must be a regular file: {auth_file_path}")
        if auth_file_path.stat().st_mode & 0o077:
            raise SandboxError("auth file must not be group- or world-accessible")
        if auth_file_path.stat().st_uid != os.getuid():
            raise SandboxError("auth file must be owned by the current user")
        if private_profile_path is not None and _inside(private_profile_path, auth_file_path):
            raise SandboxError("auth file must be separate from the private profile directory")
    if mode == "agent":
        codex_tool = tool_paths.get("codex")
        if codex_tool is None or not codex_tool.is_file() or not os.access(codex_tool, os.X_OK):
            raise SandboxError("agent mode requires an executable --tool codex=HOSTFILE")
        with codex_tool.open("rb") as codex_stream:
            is_native = codex_tool.name == "codex" and codex_stream.read(4) == b"\x7fELF"
        if not is_native:
            raise SandboxError("agent mode requires the native Codex CLI executable")
        argv = _codex_arguments(argv)
    elif not argv:
        raise SandboxError("command argv after -- must not be empty")
    root_paths = {
        "project": project_path,
        "state": state_path,
        "output": output_path,
        **{f"dependency:{key}": value for key, value in dep_paths.items()},
        **{f"tool:{key}": value for key, value in tool_paths.items()},
    }
    if private_profile_path is not None:
        root_paths["private_profile"] = private_profile_path
    if auth_file_path is not None:
        root_paths["auth_file"] = auth_file_path
    broad_roots = {Path("/"), Path("/home"), Path("/tmp"), Path("/usr"), Path("/etc"), Path("/var"), Path("/opt"), Path.home()}
    private_roots = {home_path / name for name in (".codex", ".ssh", ".aws", ".azure", ".docker", ".gnupg", ".kube", ".config")}
    if codex_home_path is not None:
        private_roots.add(codex_home_path)
    for label, root in root_paths.items():
        if root == Path("/"):
            raise SandboxError(f"filesystem root cannot be used as {label}")
        if root in broad_roots and label != "output":
            raise SandboxError(f"broad directory cannot be used as {label}: {root}")
        if label in ("project", "state", "private_profile") and codex_home_path is not None and _inside(root, codex_home_path):
            raise SandboxError(f"{label} cannot contain CODEX_HOME: {root}")
        if label.startswith(("dependency:", "tool:")) or label == "private_profile":
            if any(_inside(private_root, root) or _inside(root, private_root) for private_root in private_roots):
                raise SandboxError(f"{label} overlaps private host path: {root}")
        if label in ("project", "state"):
            for private_root in private_roots:
                if private_root == codex_home_path and _under_codex_worktrees(root, private_root):
                    continue
                if _inside(private_root, root) or _inside(root, private_root):
                    raise SandboxError(f"{label} overlaps private host path: {root}")
    _ensure_disjoint("mount", root_paths)
    if shutil.which("bwrap") is None:
        raise SandboxError("bubblewrap (bwrap) is required")
    if mode == "agent" and codex_home_path is None:
        raise SandboxError("agent mode requires a native HOME or CODEX_HOME path")
    state_path.mkdir(parents=True, exist_ok=True, mode=0o700)
    output_path.mkdir(parents=True, mode=0o700)
    with tempfile.TemporaryDirectory(prefix="edit-sandbox-", dir="/tmp") as identity_directory:
        identity_root = Path(identity_directory)
        identity_root.chmod(0o700)
        return _execute_sandbox(
            project_path,
            state_path,
            output_path,
            timeout,
            mode,
            argv,
            dep_paths,
            tool_paths,
            protected,
            network,
            private_profile_path,
            auth_file_path,
            home_value,
            codex_value,
            home_path,
            codex_home_path,
            identity_root,
        )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="edit_sandbox.py")
    commands = parser.add_subparsers(dest="subcommand", required=True)
    run_parser = commands.add_parser("run")
    run_parser.add_argument("--project", required=True)
    run_parser.add_argument("--state", required=True)
    run_parser.add_argument("--output", required=True)
    run_parser.add_argument("--timeout", required=True, type=float)
    run_parser.add_argument("--mode", required=True, choices=("prepare", "agent", "grade"))
    run_parser.add_argument("--dependency", action="append", default=[])
    run_parser.add_argument("--tool", action="append", default=[])
    run_parser.add_argument("--protect", action="append", default=[])
    run_parser.add_argument("--network", action="store_true")
    run_parser.add_argument("--private-profile")
    run_parser.add_argument("--auth-file")
    run_parser.add_argument("argv", nargs=argparse.REMAINDER)
    return parser


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        parsed = _parser().parse_args(arguments)
        argv = list(parsed.argv)
        if argv and argv[0] == "--":
            argv.pop(0)
        report = run(
            parsed.project,
            parsed.state,
            parsed.output,
            parsed.timeout,
            parsed.mode,
            argv,
            dependencies=_parse_cli_named_paths(parsed.dependency, "--dependency"),
            tools=_parse_cli_named_paths(parsed.tool, "--tool"),
            protect=parsed.protect,
            network=parsed.network,
            private_profile=parsed.private_profile,
            auth_file=parsed.auth_file,
        )
        print(json.dumps(report, sort_keys=True))
        if report["timed_out"]:
            return 124
        status = report["exit_status"]
        if status is None:
            return 1
        if status < 0:
            return min(128 + abs(int(status)), 255)
        return min(int(status), 255)
    except SandboxError as error:
        print(json.dumps({"error": str(error)}, sort_keys=True), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
