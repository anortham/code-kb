#!/usr/bin/env python3
"""Retrieval score for the code-kb tool suite.

Runs a fixed set of real retrieval tasks against a pinned snapshot of a
repository, over a live MCP session, then reports the telemetry the server
recorded for every call.

The score answers one question: did a change to code-kb make an agent's
retrieval cheaper or more expensive? It does not measure answer quality.

Usage:
    python3 scripts/retrieval_score.py
    python3 scripts/retrieval_score.py --binary /path/to/code-kb --json

The run never reads or writes the user's telemetry database at
~/.code-kb/telemetry.db. Every child process gets a private telemetry
directory inside a temporary working directory.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import shutil
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_REPO = SCRIPT_DIR.parent
DEFAULT_TASKS = SCRIPT_DIR / "retrieval_score_tasks.json"
CORPUS_DIR_NAME = "corpus"
ROW_COLUMNS = (
    "tool",
    "duration_ms",
    "outcome",
    "est_tokens",
    "est_tokens_saved",
    "est_tokens_saved_known",
    "result_count",
    "result_count_known",
)


class ScoreError(RuntimeError):
    pass


def percentile(samples: list[float], fraction: float) -> float:
    if not samples:
        return 0.0
    ordered = sorted(samples)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


class McpSession:
    """One live stdio JSON-RPC session against `code-kb serve`."""

    def __init__(self, binary: Path, corpus: Path, telemetry_dir: Path, stderr_log: Path):
        self.corpus = corpus
        self.stderr_log = stderr_log
        self._stderr_handle = stderr_log.open("w", encoding="utf-8")
        env = {**os.environ, "CODE_KB_TELEMETRY_DIR": str(telemetry_dir)}
        self.proc = subprocess.Popen(
            [str(binary), "serve"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr_handle,
            text=True,
            cwd=str(corpus),
            env=env,
        )
        self.next_id = 1

    def _send(self, message: dict[str, Any]) -> None:
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def _recv(self) -> dict[str, Any]:
        line = self.proc.stdout.readline()
        if not line:
            raise ScoreError(f"the server closed the connection; see {self.stderr_log}")
        return json.loads(line)

    def _request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            message["params"] = params
        self._send(message)
        response = self._recv()
        if response.get("id") != request_id:
            raise ScoreError(f"{method} answered request id {response.get('id')}, expected {request_id}")
        if "error" in response:
            raise ScoreError(f"{method} failed at the protocol level: {response['error']}")
        return response.get("result") or {}

    def initialize(self) -> None:
        self._request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "retrieval_score", "version": "1"},
            },
        )
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def call_tool(self, tool: str, arguments: dict[str, Any]) -> dict[str, Any]:
        payload = {"project_root": str(self.corpus), **arguments}
        return self._request("tools/call", {"name": tool, "arguments": payload})

    def close(self) -> None:
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()
            self.proc.wait(timeout=10)
        finally:
            self._stderr_handle.close()


def run_binary(binary: Path, args: list[str], telemetry_dir: Path, cwd: Path) -> str:
    env = {**os.environ, "CODE_KB_TELEMETRY_DIR": str(telemetry_dir)}
    completed = subprocess.run(
        [str(binary), *args],
        capture_output=True,
        text=True,
        env=env,
        cwd=str(cwd),
    )
    if completed.returncode != 0:
        raise ScoreError(f"`code-kb {' '.join(args)}` exited {completed.returncode}: {completed.stderr.strip()}")
    return completed.stdout


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def repo_head(repo: Path) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
    )
    return completed.stdout.strip() if completed.returncode == 0 else "unknown"


def materialize_corpus(repo: Path, commit: str, destination: Path) -> None:
    destination.mkdir(parents=True)
    archive = subprocess.run(
        ["git", "-C", str(repo), "archive", commit],
        capture_output=True,
    )
    if archive.returncode != 0:
        raise ScoreError(
            f"`git archive {commit}` failed in {repo}: {archive.stderr.decode(errors='replace').strip()}"
        )
    extract = subprocess.run(["tar", "-x", "-C", str(destination)], input=archive.stdout, capture_output=True)
    if extract.returncode != 0:
        raise ScoreError(f"tar failed to extract the corpus: {extract.stderr.decode(errors='replace').strip()}")


def read_telemetry_rows(telemetry_dir: Path, copy_dir: Path) -> list[dict[str, Any]]:
    shutil.copytree(telemetry_dir, copy_dir)
    connection = sqlite3.connect(copy_dir / "telemetry.db")
    try:
        cursor = connection.execute(f"SELECT {', '.join(ROW_COLUMNS)} FROM tool_telemetry ORDER BY rowid")
        return [dict(zip(ROW_COLUMNS, row)) for row in cursor.fetchall()]
    finally:
        connection.close()


def flatten_calls(tasks: list[dict[str, Any]]) -> list[tuple[dict[str, Any], dict[str, Any]]]:
    return [(task, call) for task in tasks for call in task["calls"]]


def score_task(task: dict[str, Any], rows_by_pass: list[list[dict[str, Any]]]) -> dict[str, Any]:
    tokens_per_pass = [sum(row["est_tokens"] for row in rows) for rows in rows_by_pass]
    saved_per_pass = [sum(row["est_tokens_saved"] for row in rows) for rows in rows_by_pass]
    durations = [float(row["duration_ms"]) for rows in rows_by_pass for row in rows]
    first = rows_by_pass[0]
    return {
        "id": task["id"],
        "question": task["question"],
        "tools": [call["tool"] for call in task["calls"]],
        "calls": len(first),
        "empty_calls": sum(1 for row in first if row["outcome"] == "empty"),
        "error_calls": sum(1 for row in first if row["outcome"] == "error"),
        "tokens_returned": tokens_per_pass[0],
        "tokens_saved": saved_per_pass[0],
        "saved_known_calls": sum(1 for row in first if row["est_tokens_saved_known"]),
        "tokens_stable": len(set(tokens_per_pass)) == 1,
        "tokens_per_pass": tokens_per_pass,
        "p50_ms": round(percentile(durations, 0.50), 2),
        "p95_ms": round(percentile(durations, 0.95), 2),
        "max_ms": round(max(durations), 2),
    }


def build_score(
    tasks: list[dict[str, Any]],
    rows: list[dict[str, Any]],
    scored_passes: int,
    warmup_passes: int,
) -> dict[str, Any]:
    flat = flatten_calls(tasks)
    calls_per_pass = len(flat)
    total_passes = scored_passes + warmup_passes
    expected = calls_per_pass * total_passes
    if len(rows) != expected:
        raise ScoreError(
            f"telemetry holds {len(rows)} rows but the run made {expected} tool calls; "
            "the score cannot map rows to tasks"
        )

    passes = [rows[index * calls_per_pass : (index + 1) * calls_per_pass] for index in range(total_passes)]
    for pass_rows in passes:
        for (_, call), row in zip(flat, pass_rows):
            if row["tool"] != call["tool"]:
                raise ScoreError(
                    f"telemetry row order does not match the call order: expected {call['tool']}, saw {row['tool']}"
                )

    latency_passes = passes[warmup_passes:]
    task_scores = []
    offset = 0
    for task in tasks:
        width = len(task["calls"])
        token_slices = [pass_rows[offset : offset + width] for pass_rows in passes]
        latency_slices = [pass_rows[offset : offset + width] for pass_rows in latency_passes]
        merged = score_task(task, token_slices)
        latency_only = score_task(task, latency_slices)
        merged["p50_ms"] = latency_only["p50_ms"]
        merged["p95_ms"] = latency_only["p95_ms"]
        merged["max_ms"] = latency_only["max_ms"]
        task_scores.append(merged)
        offset += width

    scored_rows = [row for pass_rows in latency_passes for row in pass_rows]
    durations = [float(row["duration_ms"]) for row in scored_rows]
    totals = {
        "tasks": len(tasks),
        "calls_per_pass": calls_per_pass,
        "warmup_passes": warmup_passes,
        "scored_passes": scored_passes,
        "scored_calls": len(scored_rows),
        "tokens_returned": sum(score["tokens_returned"] for score in task_scores),
        "tokens_saved": sum(score["tokens_saved"] for score in task_scores),
        "saved_known_calls": sum(score["saved_known_calls"] for score in task_scores),
        "empty_calls": sum(score["empty_calls"] for score in task_scores),
        "error_calls": sum(score["error_calls"] for score in task_scores),
        "p50_ms": round(percentile(durations, 0.50), 2),
        "p95_ms": round(percentile(durations, 0.95), 2),
        "max_ms": round(max(durations), 2),
        "mean_ms": round(statistics.fmean(durations), 2),
        "tokens_unstable_tasks": [score["id"] for score in task_scores if not score["tokens_stable"]],
    }
    totals["tokens_per_task"] = round(totals["tokens_returned"] / totals["tasks"], 1)
    totals["calls_per_task"] = round(totals["calls_per_pass"] / totals["tasks"], 2)
    totals["empty_share_pct"] = round(100.0 * totals["empty_calls"] / totals["calls_per_pass"], 1)
    return {"tasks": task_scores, "totals": totals}


def digest(score: dict[str, Any]) -> str:
    stable = [
        [task["id"], task["calls"], task["empty_calls"], task["error_calls"], task["tokens_returned"]]
        for task in score["tasks"]
    ]
    payload = json.dumps(stable, sort_keys=True).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()[:16]


def render(score: dict[str, Any], meta: dict[str, Any], cross_check: dict[str, Any]) -> str:
    lines: list[str] = []
    add = lines.append

    add("code-kb retrieval score")
    add("=======================")
    add(f"binary          {meta['binary']}")
    add(f"version         {meta['version']}")
    add(f"binary sha256   {meta['binary_sha256']}")
    add(f"corpus repo     {meta['repo']}")
    add(f"corpus commit   {meta['commit']}  (pinned; it does not follow the repo HEAD)")
    add(f"repo HEAD now   {meta['repo_head']}  (context only, not measured)")
    add(f"task set        {meta['tasks_file']}")
    add(f"index build     {meta['scan_seconds']} s (excluded from the call latency below)")
    add(f"score digest    {meta['digest']}   (identical digest = identical token and call counts)")
    add("")

    totals = score["totals"]
    add("Sample size")
    add("-----------")
    add(f"tasks                {totals['tasks']}")
    add(f"calls per pass       {totals['calls_per_pass']}")
    add(f"warm-up passes       {totals['warmup_passes']} (discarded from latency, checked for token equality)")
    add(f"scored passes        {totals['scored_passes']}")
    add(f"scored calls         {totals['scored_calls']} (the latency sample)")
    add("")

    add("Per task")
    add("--------")
    header = f"{'task':24} {'calls':>5} {'empty':>5} {'err':>4} {'tokens':>8} {'saved':>8} {'p50 ms':>8} {'p95 ms':>8}"
    add(header)
    add("-" * len(header))
    for task in score["tasks"]:
        flag = "" if task["tokens_stable"] else "  <-- TOKENS VARIED BETWEEN PASSES"
        add(
            f"{task['id']:24} {task['calls']:>5} {task['empty_calls']:>5} {task['error_calls']:>4} "
            f"{task['tokens_returned']:>8} {task['tokens_saved']:>8} {task['p50_ms']:>8.2f} {task['p95_ms']:>8.2f}{flag}"
        )
    add("-" * len(header))
    add(
        f"{'TOTAL':24} {totals['calls_per_pass']:>5} {totals['empty_calls']:>5} {totals['error_calls']:>4} "
        f"{totals['tokens_returned']:>8} {totals['tokens_saved']:>8} {totals['p50_ms']:>8.2f} {totals['p95_ms']:>8.2f}"
    )
    add("")

    add("Per task averages")
    add("-----------------")
    add(f"tokens returned per task   {totals['tokens_per_task']}")
    add(f"calls per task             {totals['calls_per_task']}")
    add(f"empty calls                {totals['empty_calls']} of {totals['calls_per_pass']} ({totals['empty_share_pct']}%)")
    add(f"error calls                {totals['error_calls']} of {totals['calls_per_pass']}")
    add(f"latency mean / p50 / p95 / max   {totals['mean_ms']} / {totals['p50_ms']} / {totals['p95_ms']} / {totals['max_ms']} ms")
    add(
        f"tokens saved is known for   {totals['saved_known_calls']} of {totals['calls_per_pass']} calls; "
        "the rest report 0 saved because no baseline size exists"
    )
    if totals["tokens_unstable_tasks"]:
        add(f"UNSTABLE TOKEN COUNTS      {', '.join(totals['tokens_unstable_tasks'])}")
    else:
        add("token counts               identical in every pass of this run")
    add("")

    add("Cross-check against `code-kb stats --json`")
    add("------------------------------------------")
    add("`stats` reports the whole private telemetry database, so it counts the warm-up pass too.")
    add(f"stats total_calls            {cross_check['stats_total_calls']}  (rows read by this script: {cross_check['rows_read']})")
    add(f"stats total_tokens_returned  {cross_check['stats_total_tokens']}")
    add(f"stats empty_calls            {cross_check['stats_empty_calls']}")
    add(f"stats error_calls            {cross_check['stats_error_calls']}")
    add(f"agreement                    {'yes' if cross_check['agrees'] else 'NO - investigate before trusting this score'}")
    add("")

    add("What this score proves")
    add("----------------------")
    add("- It proves how many tokens the code-kb tools RETURN for a fixed set of retrieval tasks,")
    add("  how many calls each task costs, how many calls come back empty or as an error,")
    add("  and how slow the calls are at the middle and at the tail.")
    add("- It compares two code-kb binaries fairly, because the corpus is pinned to one commit.")
    add("")
    add("What this score does not prove")
    add("------------------------------")
    add("- It does not prove the answer is correct or useful. No task checks the content of a result.")
    add("- It does not measure how much of a result an agent READS. Telemetry records tokens")
    add("  returned and estimated tokens saved. A result an agent skips costs the same here as one it uses.")
    add("- `empty_calls` undercounts waste. A miss that answers nothing but returns 20 unrelated")
    add("  full-text matches is recorded as `ok`, because `outcome` is `empty` only when the")
    add("  logical result count is 0. See tasks T11 and T12.")
    add("- `tokens_saved` is an estimate of file bytes avoided divided by four. It is only known for")
    add("  the calls counted above; treat it as a direction, not a measurement.")
    add("- The sample is small. 12 tasks on one repository support a weak claim about one repository.")
    add("")
    add("Smallest change that would close the reading gap")
    add("-----------------------------------------------")
    add("Record, per call, how many of the returned result rows the agent used next. The smallest")
    add("version needs no model change: give every result row a stable id (lookup and search already")
    add("print `id=<symbol_id>`), then record on each follow-up call the id it came from. One new")
    add("nullable telemetry column, `source_symbol_id`, turns 'tokens returned' into 'tokens returned")
    add("that led to a next call'. That is a code change; it belongs to Naomi Nagata, not to this script.")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="Score the code-kb retrieval tools on a fixed task set.")
    parser.add_argument("--binary", type=Path, default=None, help="code-kb binary to score (default <repo>/target/release/code-kb)")
    parser.add_argument("--repo", type=Path, default=DEFAULT_REPO, help="git repository that holds the corpus commit")
    parser.add_argument("--tasks", type=Path, default=DEFAULT_TASKS, help="task set JSON file")
    parser.add_argument("--commit", default=None, help="corpus commit (default: the commit pinned in the task set)")
    parser.add_argument("--passes", type=int, default=3, help="scored passes over the task set (default 3)")
    parser.add_argument("--json", action="store_true", help="print the score as JSON instead of a table")
    parser.add_argument("--keep", action="store_true", help="keep the temporary working directory and print its path")
    args = parser.parse_args()

    tasks_file = args.tasks.resolve()
    task_set = json.loads(tasks_file.read_text(encoding="utf-8"))
    tasks = task_set["tasks"]
    commit = args.commit or task_set["corpus"]["commit"]
    repo = args.repo.resolve()
    binary = (args.binary or repo / "target" / "release" / "code-kb").resolve()
    if not binary.is_file():
        raise ScoreError(f"no code-kb binary at {binary}; build it with `cargo build --release`")
    if args.passes < 1:
        raise ScoreError("--passes must be at least 1")

    work_dir = Path(tempfile.mkdtemp(prefix="code_kb_retrieval_score_"))
    try:
        corpus = work_dir / CORPUS_DIR_NAME
        telemetry_dir = work_dir / "telemetry"
        telemetry_dir.mkdir()
        materialize_corpus(repo, commit, corpus)

        version = run_binary(binary, ["--version"], telemetry_dir, work_dir).strip()
        scan_started = time.perf_counter()
        run_binary(binary, ["--root", str(corpus), "scan"], telemetry_dir, work_dir)
        scan_seconds = round(time.perf_counter() - scan_started, 2)

        session = McpSession(binary, corpus, telemetry_dir, work_dir / "serve.stderr.log")
        try:
            session.initialize()
            for _ in range(args.passes + 1):
                for task in tasks:
                    for call in task["calls"]:
                        session.call_tool(call["tool"], call["arguments"])
        finally:
            session.close()

        rows = read_telemetry_rows(telemetry_dir, work_dir / "telemetry-copy")
        score = build_score(tasks, rows, scored_passes=args.passes, warmup_passes=1)

        stats = json.loads(run_binary(binary, ["stats", "--json"], telemetry_dir, work_dir))
        cross_check = {
            "stats_total_calls": stats["total_calls"],
            "stats_total_tokens": stats["total_tokens_returned"],
            "stats_empty_calls": stats["empty_calls"],
            "stats_error_calls": stats["error_calls"],
            "rows_read": len(rows),
            "agrees": stats["total_calls"] == len(rows)
            and stats["total_tokens_returned"] == sum(row["est_tokens"] for row in rows),
        }
        meta = {
            "binary": str(binary),
            "version": version,
            "binary_sha256": file_sha256(binary),
            "repo": str(repo),
            "repo_head": repo_head(repo),
            "commit": commit,
            "tasks_file": str(tasks_file),
            "scan_seconds": scan_seconds,
            "digest": digest(score),
        }

        if args.json:
            print(json.dumps({"meta": meta, "score": score, "cross_check": cross_check}, indent=2))
        else:
            print(render(score, meta, cross_check))
        if args.keep:
            print(f"\nworking directory kept at {work_dir}")
        return 0
    finally:
        if not args.keep:
            shutil.rmtree(work_dir, ignore_errors=True)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except ScoreError as error:
        print(f"retrieval score failed: {error}", file=sys.stderr)
        sys.exit(2)
