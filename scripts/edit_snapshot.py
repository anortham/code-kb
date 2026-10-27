import hashlib
import json
import ntpath
import os
import re
import shutil
import stat
import uuid
from pathlib import Path, PurePath


SCHEMA = 1
DEFAULT_MAX_BYTES = 33_554_432
MAX_ENTRIES = 100_000
MAX_CHANGES = MAX_ENTRIES * 2
MAX_BUNDLE_BYTES = 67_108_864
MAX_FILE_BYTES = 134_217_728
MAX_SCAN_BYTES = 1_073_741_824
MAX_SYMLINK_DEPTH = 128
CHUNK_SIZE = 1024 * 1024
HEX_SHA256 = re.compile(r"[0-9a-f]{64}\Z")


class SnapshotError(ValueError):
    pass


def snapshot_tree(root, *, excluded=()):
    """Return a deterministic schema-1 manifest for a project tree."""
    excluded_paths = _trusted_paths(excluded, "excluded")
    entries, violations = _scan_tree(root, excluded_paths)
    violations.extend(_entry_violations(entries))
    if violations:
        _raise_first_violation(violations)
    return _make_snapshot(entries)


def capture_bundle(seed_manifest, root, output, *, protected=(), excluded=(), max_bytes=DEFAULT_MAX_BYTES):
    """Write a replayable content bundle, retaining violations as evidence."""
    if isinstance(max_bytes, bool) or not isinstance(max_bytes, int) or not 0 <= max_bytes <= DEFAULT_MAX_BYTES:
        raise ValueError(f"max_bytes must be between 0 and {DEFAULT_MAX_BYTES}")
    protected_paths, excluded_paths = _validated_paths(protected, excluded)
    seed = _validate_snapshot(seed_manifest, "seed snapshot")
    _check_excluded_manifest(seed["entries"], excluded_paths, "seed snapshot")
    root_path = _absolute_path(root)
    output_input = _input_absolute_path(output)
    if _path_exists(output_input):
        raise FileExistsError(output_input)
    output_path = output_input.resolve(strict=False)
    if _paths_overlap(root_path, output_path):
        raise ValueError("bundle output must be outside the captured tree")
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.mkdir()
    blob_root = output_path / "blobs"
    blob_root.mkdir()

    entries, violations = _scan_tree(root_path, excluded_paths)
    violations.extend(_entry_violations(entries))
    final_snapshot = _make_snapshot(entries)
    seed_entries = {entry["path"]: entry for entry in seed["entries"]}
    final_entries = {entry["path"]: entry for entry in final_snapshot["entries"]}
    changes = []
    for path in sorted(seed_entries.keys() | final_entries.keys()):
        before = seed_entries.get(path)
        after = final_entries.get(path)
        if before != after:
            changes.append({"path": path, "before": before, "after": after})
            if _affects_protected(path, protected_paths):
                _add_violation(violations, "protected_path", path, "protected path changed")

    _write_changed_blobs(changes, root_path, blob_root, max_bytes, violations)
    bundle = {
        "schema": SCHEMA,
        "seed_digest": seed["digest"],
        "final_digest": final_snapshot["digest"],
        "changes": changes,
        "violations": _sorted_violations(violations),
    }
    _write_json_exclusive(output_path / "bundle.json", bundle)
    return bundle


def apply_bundle(seed_root, bundle, destination, *, protected=(), excluded=()):
    """Validate and replay a bundle into a destination that does not exist."""
    protected_paths, excluded_paths = _validated_paths(protected, excluded)
    seed_path = _required_directory(seed_root, "seed root")
    bundle_path = _required_directory(bundle, "bundle")
    destination_input = _input_absolute_path(destination)
    if _path_exists(destination_input):
        raise FileExistsError(destination_input)
    destination_path = destination_input.resolve(strict=False)
    if destination_path.exists() or destination_path.is_symlink():
        raise FileExistsError(destination_path)
    if _paths_overlap(seed_path, destination_path):
        raise SnapshotError("replay destination overlaps the seed tree")
    if _paths_overlap(bundle_path, destination_path):
        raise SnapshotError("replay destination overlaps the bundle")

    bundle_data = _read_bundle(bundle_path)
    parsed = _validate_bundle(bundle_data)
    if parsed["violations"]:
        _raise_first_violation(parsed["violations"])

    seed_snapshot = snapshot_tree(seed_path, excluded=excluded_paths)
    _check_excluded_manifest(parsed["changes"], excluded_paths, "bundle changes")
    if parsed["seed_digest"] != seed_snapshot["digest"]:
        raise SnapshotError("seed identity does not match the bundle")

    seed_entries = {entry["path"]: entry for entry in seed_snapshot["entries"]}
    final_entries = dict(seed_entries)
    changes_by_path = {}
    for change in parsed["changes"]:
        path = change["path"]
        if change["before"] != seed_entries.get(path):
            raise SnapshotError(f"bundle before-entry does not match the seed at {path}")
        if _affects_protected(path, protected_paths):
            raise SnapshotError(f"bundle changes protected path {path}")
        if change["after"] is None:
            final_entries.pop(path, None)
        else:
            final_entries[path] = change["after"]
        changes_by_path[path] = change

    expected_snapshot = _make_snapshot(sorted(final_entries.values(), key=lambda entry: entry["path"]))
    _validate_snapshot(expected_snapshot, "replayed snapshot")
    if expected_snapshot["digest"] != parsed["final_digest"]:
        raise SnapshotError("bundle final identity does not match its changes")

    blob_paths = _validate_blob_storage(bundle_path, parsed["required_blobs"])
    for name, info in blob_paths.items():
        actual_hash, actual_size = _hash_regular_file(info)
        if actual_hash != name or actual_size != parsed["blob_sizes"][name]:
            raise SnapshotError(f"blob {name} failed its content check")

    destination_path.parent.mkdir(parents=True, exist_ok=True)
    destination_path.mkdir()
    try:
        _materialize_tree(seed_path, destination_path, expected_snapshot["entries"], seed_entries, changes_by_path, blob_paths)
        replayed = snapshot_tree(destination_path, excluded=excluded_paths)
        if replayed != expected_snapshot:
            raise SnapshotError("replayed tree does not match the bundle final manifest")
    except Exception:
        shutil.rmtree(destination_path, ignore_errors=True)
        raise
    return replayed


def _validated_paths(protected, excluded):
    protected_paths = _trusted_paths(protected, "protected")
    excluded_paths = _trusted_paths(excluded, "excluded")
    for protected_path in protected_paths:
        for excluded_path in excluded_paths:
            if _path_intersects(protected_path, excluded_path):
                raise ValueError(f"excluded path {excluded_path} overlaps protected path {protected_path}")
    return protected_paths, excluded_paths


def _trusted_paths(values, label):
    if values is None:
        return ()
    if isinstance(values, (str, os.PathLike)):
        values = (values,)
    try:
        result = {_relative_path(value, label) for value in values}
    except TypeError as exc:
        raise ValueError(f"{label} must be a path or an iterable of paths") from exc
    return tuple(sorted(result))


def _relative_path(value, label):
    if isinstance(value, PurePath):
        value = value.as_posix()
    elif isinstance(value, os.PathLike):
        value = os.fspath(value)
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label} paths must be non-empty relative strings")
    drive, _ = ntpath.splitdrive(value)
    parts = value.split("/")
    if drive or value.startswith("/") or "\\" in value or "\x00" in value or any(part in ("", ".", "..") for part in parts):
        raise ValueError(f"{label} path is not a safe relative path: {value!r}")
    return value


def _absolute_path(value):
    try:
        return _input_absolute_path(value).resolve(strict=False)
    except (OSError, RuntimeError, TypeError, ValueError) as exc:
        raise ValueError(f"invalid filesystem path: {value!r}") from exc


def _input_absolute_path(value):
    try:
        return Path(value).expanduser().absolute()
    except (OSError, RuntimeError, TypeError, ValueError) as exc:
        raise ValueError(f"invalid filesystem path: {value!r}") from exc


def _path_exists(path):
    try:
        path.lstat()
        return True
    except FileNotFoundError:
        return False


def _required_directory(value, label):
    path = _input_absolute_path(value)
    try:
        mode = path.lstat().st_mode
    except OSError as exc:
        raise SnapshotError(f"{label} does not exist") from exc
    if not stat.S_ISDIR(mode):
        raise SnapshotError(f"{label} must be a real directory")
    return path.resolve(strict=True)


def _paths_overlap(first, second):
    try:
        first.relative_to(second)
        return True
    except ValueError:
        pass
    try:
        second.relative_to(first)
        return True
    except ValueError:
        return False


def _path_at_or_under(path, parent):
    return path == parent or path.startswith(parent + "/")


def _path_intersects(first, second):
    return _path_at_or_under(first, second) or _path_at_or_under(second, first)


def _affects_protected(path, protected):
    return any(_path_intersects(path, item) for item in protected)


def _check_excluded_manifest(entries, excluded, label):
    for entry in entries:
        path = entry["path"] if isinstance(entry, dict) else entry
        if any(_path_at_or_under(path, excluded_path) for excluded_path in excluded):
            raise SnapshotError(f"{label} includes excluded path {path}")


def _excluded(path, excluded):
    return any(_path_at_or_under(path, item) for item in excluded)


def _scan_tree(root, excluded):
    entries = []
    violations = []
    try:
        root_path = _absolute_path(root)
    except (OSError, RuntimeError, TypeError, ValueError) as exc:
        _add_violation(violations, "root_unavailable", ".", _safe_error_message(exc))
        return entries, violations
    try:
        root_stat = root_path.stat()
    except OSError as exc:
        _add_violation(violations, "root_unavailable", ".", _os_error_message(exc))
        return entries, violations
    if not stat.S_ISDIR(root_stat.st_mode):
        _add_violation(violations, "root_not_directory", ".", "source root is not a directory")
        return entries, violations

    pending = [(root_path, "")]
    seen = 0
    scanned_bytes = 0
    overflow = False
    while pending and not overflow:
        directory, prefix = pending.pop()
        try:
            iterator = os.scandir(directory)
        except OSError as exc:
            _add_violation(violations, "directory_unavailable", prefix or ".", _os_error_message(exc))
            continue
        with iterator:
            while True:
                try:
                    item = next(iterator)
                except StopIteration:
                    break
                except OSError as exc:
                    _add_violation(violations, "directory_unavailable", prefix or ".", _os_error_message(exc))
                    break
                path = f"{prefix}/{item.name}" if prefix else item.name
                if _excluded(path, excluded):
                    continue
                seen += 1
                if seen > MAX_ENTRIES:
                    _add_violation(violations, "entry_limit", path, f"tree exceeds {MAX_ENTRIES} entries")
                    overflow = True
                    break
                try:
                    _relative_path(path, "tree entry")
                except ValueError:
                    _add_violation(violations, "unsafe_path", path, "tree entry is not a portable relative path")
                    continue
                try:
                    info = item.stat(follow_symlinks=False)
                except OSError as exc:
                    _add_violation(violations, "entry_unavailable", path, _os_error_message(exc))
                    continue
                if stat.S_ISDIR(info.st_mode):
                    entries.append({"path": path, "kind": "directory"})
                    pending.append((Path(item.path), path))
                elif stat.S_ISREG(info.st_mode):
                    if info.st_size > MAX_FILE_BYTES:
                        _add_violation(violations, "file_size_limit", path, f"file exceeds {MAX_FILE_BYTES} bytes")
                        continue
                    remaining_bytes = MAX_SCAN_BYTES - scanned_bytes
                    if info.st_size > remaining_bytes:
                        _add_violation(violations, "scan_size_limit", path, f"tree exceeds {MAX_SCAN_BYTES} hashed bytes")
                        overflow = True
                        break
                    try:
                        file_hash, file_size = _hash_regular_file(
                            Path(item.path), expected_stat=info, max_bytes=min(MAX_FILE_BYTES, remaining_bytes)
                        )
                    except (OSError, SnapshotError) as exc:
                        _add_violation(violations, "file_unavailable", path, _safe_error_message(exc))
                        continue
                    scanned_bytes += file_size
                    entries.append(
                        {
                            "path": path,
                            "kind": "file",
                            "sha256": file_hash,
                            "size": file_size,
                            "executable": bool(info.st_mode & 0o111),
                        }
                    )
                elif stat.S_ISLNK(info.st_mode):
                    try:
                        target = os.readlink(item.path)
                    except OSError as exc:
                        _add_violation(violations, "link_unavailable", path, _os_error_message(exc))
                        continue
                    entries.append({"path": path, "kind": "symlink", "target": target})
                else:
                    _add_violation(violations, "special_file", path, "special filesystem entries are not supported")
    entries.sort(key=lambda entry: entry["path"])
    return entries, _sorted_violations(violations)


def _entry_violations(entries):
    violations = []
    paths = {entry["path"]: entry for entry in entries}
    for entry in entries:
        path = entry["path"]
        parts = path.split("/")
        for index in range(1, len(parts)):
            parent_path = "/".join(parts[:index])
            parent = paths.get(parent_path)
            if parent is None or parent["kind"] != "directory":
                _add_violation(violations, "invalid_parent", path, f"parent {parent_path} is not a directory")
                break
    for entry in entries:
        if entry["kind"] != "symlink":
            continue
        issue = _resolve_link(entry["path"], entry["target"], paths, {entry["path"]})
        if issue:
            _add_violation(violations, issue[0], entry["path"], issue[1])
    return _sorted_violations(violations)


def _resolve_link(link_path, target, entries, active):
    if not isinstance(target, str) or not target or "\x00" in target or "\\" in target:
        return "unsafe_link", "symlink target must be a non-empty relative path"
    drive, _ = ntpath.splitdrive(target)
    if drive or target.startswith("/"):
        return "unsafe_link", "absolute symlink targets are not supported"

    def expand(base, components, resolving):
        stack = list(base)
        for component in components:
            if component in ("", "."):
                continue
            if component == "..":
                if not stack:
                    return None, ("escaping_link", "symlink target escapes the project root")
                stack.pop()
                continue
            candidate = "/".join((*stack, component))
            nested = entries.get(candidate)
            if nested is not None and nested["kind"] == "symlink":
                if candidate in resolving:
                    return None, ("symlink_cycle", "symlink target contains a cycle")
                if len(resolving) >= MAX_SYMLINK_DEPTH:
                    return None, ("symlink_depth_limit", f"symlink chain exceeds {MAX_SYMLINK_DEPTH} links")
                nested_target = nested["target"]
                nested_drive, _ = ntpath.splitdrive(nested_target) if isinstance(nested_target, str) else ("", "")
                if not isinstance(nested_target, str) or not nested_target or nested_drive or nested_target.startswith("/") or "\\" in nested_target or "\x00" in nested_target:
                    return None, ("unsafe_link", "symlink chain contains an unsupported target")
                resolved, issue = expand(stack, nested_target.split("/"), resolving | {candidate})
                if issue:
                    return None, issue
                stack = resolved
            else:
                stack.append(component)
        return stack, None

    base = link_path.split("/")[:-1]
    _, issue = expand(base, target.split("/"), active)
    return issue


def _make_snapshot(entries):
    ordered = sorted(entries, key=lambda entry: entry["path"])
    return {"schema": SCHEMA, "entries": ordered, "digest": _entries_digest(ordered)}


def _entries_digest(entries):
    encoded = json.dumps(entries, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _validate_snapshot(snapshot, label):
    if not isinstance(snapshot, dict) or set(snapshot) != {"schema", "entries", "digest"}:
        raise SnapshotError(f"{label} has an invalid schema")
    if type(snapshot["schema"]) is not int or snapshot["schema"] != SCHEMA:
        raise SnapshotError(f"{label} uses an unsupported schema")
    entries = snapshot["entries"]
    if not isinstance(entries, list) or len(entries) > MAX_ENTRIES:
        raise SnapshotError(f"{label} exceeds the entry limit or has invalid entries")
    paths = []
    total_file_bytes = 0
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("path"), str):
            raise SnapshotError(f"{label} contains an invalid entry")
        try:
            path = _relative_path(entry["path"], label)
        except ValueError as exc:
            raise SnapshotError(str(exc)) from exc
        if path != entry["path"]:
            raise SnapshotError(f"{label} contains a non-canonical path")
        kind = entry.get("kind")
        if kind == "directory":
            if set(entry) != {"path", "kind"}:
                raise SnapshotError(f"{label} contains an invalid directory entry at {path}")
        elif kind == "file":
            if set(entry) != {"path", "kind", "sha256", "size", "executable"}:
                raise SnapshotError(f"{label} contains an invalid file entry at {path}")
            if not isinstance(entry["sha256"], str) or not HEX_SHA256.fullmatch(entry["sha256"]):
                raise SnapshotError(f"{label} contains an invalid file hash at {path}")
            if type(entry["size"]) is not int or entry["size"] < 0 or type(entry["executable"]) is not bool:
                raise SnapshotError(f"{label} contains invalid file metadata at {path}")
            if entry["size"] > MAX_FILE_BYTES:
                raise SnapshotError(f"{label} file exceeds {MAX_FILE_BYTES} bytes at {path}")
            total_file_bytes += entry["size"]
            if total_file_bytes > MAX_SCAN_BYTES:
                raise SnapshotError(f"{label} exceeds {MAX_SCAN_BYTES} total file bytes")
        elif kind == "symlink":
            if set(entry) != {"path", "kind", "target"} or not isinstance(entry["target"], str):
                raise SnapshotError(f"{label} contains an invalid symlink entry at {path}")
        else:
            raise SnapshotError(f"{label} contains an unsupported entry kind at {path}")
        paths.append(path)
    if paths != sorted(paths) or len(paths) != len(set(paths)):
        raise SnapshotError(f"{label} entries must be unique and sorted")
    issues = _entry_violations(entries)
    if issues:
        _raise_first_violation(issues)
    if not isinstance(snapshot["digest"], str) or not HEX_SHA256.fullmatch(snapshot["digest"]):
        raise SnapshotError(f"{label} has an invalid digest")
    if snapshot["digest"] != _entries_digest(entries):
        raise SnapshotError(f"{label} digest does not match its entries")
    return snapshot


def _write_changed_blobs(changes, root, blob_root, max_bytes, violations):
    payloads = {}
    for change in changes:
        before = change["before"]
        after = change["after"]
        if after is None or after.get("kind") != "file":
            continue
        if before is not None and before.get("kind") == "file" and before["sha256"] == after["sha256"]:
            continue
        payloads.setdefault(after["sha256"], (change["path"], after["size"]))

    used_bytes = 0
    skipped = set()
    for digest, (relative, size) in sorted(payloads.items()):
        target = blob_root / digest
        if target.exists():
            continue
        if digest in skipped:
            continue
        if size > max_bytes or used_bytes + size > max_bytes:
            skipped.add(digest)
            _add_violation(violations, "payload_limit", relative, f"changed payload exceeds {max_bytes} bytes")
            continue
        temporary = blob_root / f".{digest}.{uuid.uuid4().hex}.tmp"
        try:
            _copy_regular_verified(_join_relative(root, relative), temporary, digest, size)
            os.replace(temporary, target)
            used_bytes += size
        except (OSError, SnapshotError) as exc:
            try:
                temporary.unlink()
            except OSError:
                pass
            _add_violation(violations, "blob_capture_failed", relative, _safe_error_message(exc))


def _join_relative(root, relative):
    return root.joinpath(*relative.split("/"))


def _hash_regular_file(path, expected_stat=None, max_bytes=MAX_FILE_BYTES):
    if expected_stat is not None and expected_stat.st_size > max_bytes:
        raise SnapshotError(f"file exceeds {max_bytes} byte hash limit")
    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    with os.fdopen(descriptor, "rb") as source:
        before = os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode):
            raise SnapshotError("path is not a regular file")
        if before.st_size > max_bytes:
            raise SnapshotError(f"file exceeds {max_bytes} byte hash limit")
        if expected_stat is not None and (before.st_dev, before.st_ino) != (expected_stat.st_dev, expected_stat.st_ino):
            raise SnapshotError("file changed while it was being inspected")
        digest = hashlib.sha256()
        size = 0
        while True:
            chunk = source.read(CHUNK_SIZE)
            if not chunk:
                break
            if size + len(chunk) > max_bytes:
                raise SnapshotError(f"file exceeds {max_bytes} byte hash limit")
            digest.update(chunk)
            size += len(chunk)
        after = os.fstat(source.fileno())
        current = os.stat(path, follow_symlinks=False)
        if not _same_file_identity(before, after) or not _same_file_identity(after, current) or size != after.st_size:
            raise SnapshotError("file changed while it was being read")
    return digest.hexdigest(), size


def _same_file_identity(first, second):
    return (
        stat.S_ISREG(first.st_mode)
        and stat.S_ISREG(second.st_mode)
        and first.st_dev == second.st_dev
        and first.st_ino == second.st_ino
        and first.st_size == second.st_size
        and getattr(first, "st_mtime_ns", first.st_mtime) == getattr(second, "st_mtime_ns", second.st_mtime)
        and getattr(first, "st_ctime_ns", first.st_ctime) == getattr(second, "st_ctime_ns", second.st_ctime)
    )


def _copy_regular_verified(source_path, destination_path, expected_hash, expected_size):
    if expected_size > MAX_FILE_BYTES:
        raise SnapshotError(f"file exceeds {MAX_FILE_BYTES} bytes")
    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(source_path, flags)
    destination_descriptor = None
    try:
        source = os.fdopen(descriptor, "rb")
        descriptor = None
        before = os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode):
            raise SnapshotError("copy source is not a regular file")
        if before.st_size > MAX_FILE_BYTES:
            raise SnapshotError(f"copy source exceeds {MAX_FILE_BYTES} bytes")
        destination_descriptor = os.open(destination_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0), 0o600)
        digest = hashlib.sha256()
        size = 0
        with source, os.fdopen(destination_descriptor, "wb") as destination:
            destination_descriptor = None
            while True:
                chunk = source.read(CHUNK_SIZE)
                if not chunk:
                    break
                if size + len(chunk) > expected_size or size + len(chunk) > MAX_FILE_BYTES:
                    raise SnapshotError("copy source exceeds its declared size")
                destination.write(chunk)
                digest.update(chunk)
                size += len(chunk)
            destination.flush()
            after = os.fstat(source.fileno())
            current = os.stat(source_path, follow_symlinks=False)
            if not _same_file_identity(before, after) or not _same_file_identity(after, current) or size != after.st_size:
                raise SnapshotError("copy source changed while it was being read")
        if digest.hexdigest() != expected_hash or size != expected_size:
            raise SnapshotError("copy source failed its content check")
    except Exception:
        try:
            Path(destination_path).unlink()
        except OSError:
            pass
        raise
    finally:
        if descriptor is not None:
            os.close(descriptor)
        if destination_descriptor is not None:
            os.close(destination_descriptor)


def _validate_bundle(bundle):
    if not isinstance(bundle, dict) or set(bundle) != {"schema", "seed_digest", "final_digest", "changes", "violations"}:
        raise SnapshotError("bundle has an invalid schema")
    if type(bundle["schema"]) is not int or bundle["schema"] != SCHEMA:
        raise SnapshotError("bundle uses an unsupported schema")
    for key in ("seed_digest", "final_digest"):
        if not isinstance(bundle[key], str) or not HEX_SHA256.fullmatch(bundle[key]):
            raise SnapshotError(f"bundle has an invalid {key}")
    changes = bundle["changes"]
    if not isinstance(changes, list) or len(changes) > MAX_CHANGES:
        raise SnapshotError("bundle exceeds the change limit")
    paths = []
    required_blobs = set()
    blob_sizes = {}
    for change in changes:
        if not isinstance(change, dict) or set(change) != {"path", "before", "after"}:
            raise SnapshotError("bundle contains an invalid change")
        try:
            path = _relative_path(change["path"], "bundle change")
        except ValueError as exc:
            raise SnapshotError(str(exc)) from exc
        if path != change["path"]:
            raise SnapshotError("bundle contains a non-canonical change path")
        paths.append(path)
        before = change["before"]
        after = change["after"]
        if before is None and after is None:
            raise SnapshotError(f"bundle has an empty change at {path}")
        for label, entry in (("before", before), ("after", after)):
            if entry is None:
                continue
            _validate_single_entry(entry, path, f"bundle {label} entry")
        if before == after:
            raise SnapshotError(f"bundle has an unchanged entry at {path}")
        if after is not None and after["kind"] == "file":
            if before is None or before["kind"] != "file" or before["sha256"] != after["sha256"]:
                required_blobs.add(after["sha256"])
                previous_size = blob_sizes.setdefault(after["sha256"], after["size"])
                if previous_size != after["size"]:
                    raise SnapshotError(f"bundle assigns different sizes to blob {after['sha256']}")
    if sum(blob_sizes.values()) > DEFAULT_MAX_BYTES:
        raise SnapshotError("bundle changed payload exceeds the size limit")
    if paths != sorted(paths) or len(paths) != len(set(paths)):
        raise SnapshotError("bundle changes must be unique and sorted")
    violations = bundle["violations"]
    if not isinstance(violations, list) or len(violations) > MAX_CHANGES:
        raise SnapshotError("bundle has invalid violations")
    for violation in violations:
        if not isinstance(violation, dict) or set(violation) != {"code", "path", "message"}:
            raise SnapshotError("bundle contains an invalid violation")
        if not all(isinstance(violation[key], str) for key in ("code", "path", "message")):
            raise SnapshotError("bundle contains an invalid violation")
    return {
        "schema": SCHEMA,
        "seed_digest": bundle["seed_digest"],
        "final_digest": bundle["final_digest"],
        "changes": changes,
        "violations": violations,
        "required_blobs": required_blobs,
        "blob_sizes": blob_sizes,
    }


def _validate_single_entry(entry, path, label):
    if not isinstance(entry, dict) or entry.get("path") != path:
        raise SnapshotError(f"{label} has a mismatched path at {path}")
    kind = entry.get("kind")
    if kind == "directory":
        valid_keys = {"path", "kind"}
    elif kind == "file":
        valid_keys = {"path", "kind", "sha256", "size", "executable"}
    elif kind == "symlink":
        valid_keys = {"path", "kind", "target"}
    else:
        raise SnapshotError(f"{label} has an invalid kind at {path}")
    if set(entry) != valid_keys:
        raise SnapshotError(f"{label} has invalid fields at {path}")
    if kind == "file":
        if not isinstance(entry["sha256"], str) or not HEX_SHA256.fullmatch(entry["sha256"]):
            raise SnapshotError(f"{label} has an invalid hash at {path}")
        if type(entry["size"]) is not int or entry["size"] < 0 or type(entry["executable"]) is not bool:
            raise SnapshotError(f"{label} has invalid file metadata at {path}")
        if entry["size"] > MAX_FILE_BYTES:
            raise SnapshotError(f"{label} file exceeds {MAX_FILE_BYTES} bytes at {path}")
    if kind == "symlink":
        if not isinstance(entry["target"], str):
            raise SnapshotError(f"{label} has an invalid target at {path}")


def _read_bundle(bundle_path):
    try:
        with os.scandir(bundle_path) as items:
            top_level = {item.name: item for item in items}
    except OSError as exc:
        raise SnapshotError(f"bundle cannot be inspected: {_os_error_message(exc)}") from exc
    if set(top_level) != {"bundle.json", "blobs"}:
        raise SnapshotError("bundle must contain only bundle.json and blobs")
    json_path = bundle_path / "bundle.json"
    blobs_path = bundle_path / "blobs"
    try:
        json_stat = json_path.lstat()
        blobs_stat = blobs_path.lstat()
    except OSError as exc:
        raise SnapshotError("bundle is missing required storage") from exc
    if not stat.S_ISREG(json_stat.st_mode) or not stat.S_ISDIR(blobs_stat.st_mode):
        raise SnapshotError("bundle storage must not contain links or special files")
    if json_stat.st_size > MAX_BUNDLE_BYTES:
        raise SnapshotError("bundle manifest exceeds the size limit")
    contents = _read_regular_file(json_path, MAX_BUNDLE_BYTES)
    try:
        return json.loads(contents, object_pairs_hook=_reject_duplicate_keys)
    except (json.JSONDecodeError, UnicodeDecodeError, ValueError, RecursionError) as exc:
        raise SnapshotError("bundle manifest is not valid JSON") from exc


def _reject_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key {key}")
        result[key] = value
    return result


def _read_regular_file(path, limit):
    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    with os.fdopen(descriptor, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise SnapshotError("bundle manifest is not a bounded regular file")
        chunks = []
        size = 0
        while True:
            chunk = source.read(CHUNK_SIZE)
            if not chunk:
                break
            size += len(chunk)
            if size > limit:
                raise SnapshotError("bundle manifest exceeds the size limit")
            chunks.append(chunk)
        return b"".join(chunks)


def _validate_blob_storage(bundle_path, required_blobs):
    blob_root = bundle_path / "blobs"
    try:
        blob_root_stat = blob_root.lstat()
    except OSError as exc:
        raise SnapshotError(f"blob storage cannot be inspected: {_os_error_message(exc)}") from exc
    if not stat.S_ISDIR(blob_root_stat.st_mode):
        raise SnapshotError("blob storage must be a real directory")
    paths = {}
    total_size = 0
    try:
        with os.scandir(blob_root) as entries:
            for item in entries:
                name = item.name
                if not HEX_SHA256.fullmatch(name) or name not in required_blobs:
                    raise SnapshotError(f"bundle has an unexpected blob entry {name!r}")
                info = item.stat(follow_symlinks=False)
                if not stat.S_ISREG(info.st_mode):
                    raise SnapshotError(f"blob {name} is not a regular file")
                if info.st_size > DEFAULT_MAX_BYTES:
                    raise SnapshotError(f"blob {name} exceeds the payload limit")
                total_size += info.st_size
                if total_size > DEFAULT_MAX_BYTES:
                    raise SnapshotError("bundle changed payload exceeds the size limit")
                paths[name] = Path(item.path)
    except OSError as exc:
        raise SnapshotError(f"blob storage cannot be inspected: {_os_error_message(exc)}") from exc
    if set(paths) != required_blobs:
        raise SnapshotError("bundle is missing one or more required blobs")
    return paths


def _materialize_tree(seed_root, destination, entries, seed_entries, changes, blob_paths):
    directories = [entry for entry in entries if entry["kind"] == "directory"]
    for entry in sorted(directories, key=lambda item: (item["path"].count("/"), item["path"])):
        _join_relative(destination, entry["path"]).mkdir()

    for entry in entries:
        if entry["kind"] != "file":
            continue
        path = entry["path"]
        change = changes.get(path)
        if change is not None and entry["sha256"] in blob_paths:
            source = blob_paths[entry["sha256"]]
        else:
            source = _join_relative(seed_root, path)
        target = _join_relative(destination, path)
        _copy_regular_verified(source, target, entry["sha256"], entry["size"])
        seed_entry = seed_entries.get(path)
        if seed_entry is not None and seed_entry["kind"] == "file":
            source_info = _join_relative(seed_root, path).lstat()
            if not stat.S_ISREG(source_info.st_mode):
                raise SnapshotError(f"seed file changed type at {path}")
            mode = stat.S_IMODE(source_info.st_mode)
        else:
            mode = 0o644
        if entry["executable"]:
            mode |= 0o111
        else:
            mode &= ~0o111
        os.chmod(target, mode)

    for entry in entries:
        if entry["kind"] == "symlink":
            os.symlink(entry["target"], _join_relative(destination, entry["path"]))

    for entry in directories:
        os.chmod(_join_relative(destination, entry["path"]), 0o755)


def _write_json_exclusive(path, value):
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    data = (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=True) + "\n").encode("utf-8")
    try:
        with open(temporary, "xb") as output:
            output.write(data)
        os.replace(temporary, path)
    except Exception:
        try:
            temporary.unlink()
        except OSError:
            pass
        raise


def _add_violation(violations, code, path, message):
    violations.append({"code": code, "path": path or ".", "message": str(message)})


def _sorted_violations(violations):
    unique = {(item["code"], item["path"], item["message"]): item for item in violations}
    return [unique[key] for key in sorted(unique)]


def _raise_first_violation(violations):
    first = _sorted_violations(violations)[0]
    raise SnapshotError(f"{first['code']} at {first['path']}: {first['message']}")


def _safe_error_message(exc):
    if isinstance(exc, OSError):
        return _os_error_message(exc)
    return str(exc)


def _os_error_message(exc):
    return exc.strerror or type(exc).__name__
