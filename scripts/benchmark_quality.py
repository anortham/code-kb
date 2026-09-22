#!/usr/bin/env python3
"""
Benchmark suite for code-kb: Token efficiency, query latency, memory footprint, and search quality.
Measures real performance, token reduction, peak RSS, and search precision across code-kb operations.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import selectors
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any


def percentile(samples: list[float], fraction: float) -> float:
    ordered = sorted(samples)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def mcp_result(response: dict[str, Any], request_id: int, operation: str) -> dict[str, Any]:
    if response.get("id") != request_id:
        raise RuntimeError(f"{operation} response id did not match request id {request_id}")
    if "error" in response:
        raise RuntimeError(f"{operation} failed: {response['error']}")
    result = response.get("result")
    if not isinstance(result, dict):
        raise RuntimeError(f"{operation} returned no result")
    if result.get("isError"):
        raise RuntimeError(f"{operation} returned an MCP tool error")
    return result


def contains_symbol_declaration(result: dict[str, Any], symbol: str, path: str) -> bool:
    content = result.get("content", [])
    text = "".join(item.get("text", "") for item in content if isinstance(item, dict))
    declaration = re.compile(
        rf"^- function `{re.escape(symbol)}` \[{re.escape(Path(path).as_posix())}:\d+(?:-\d+)?\]",
        re.MULTILINE,
    )
    return declaration.search(text) is not None


def file_sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as binary_file:
        for chunk in iter(lambda: binary_file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def estimate_tokens(text: str) -> int:
    """Accurate token estimation for code and text.
    Uses tiktoken cl100k_base if available; falls back to a regex-based BPE proxy
    that properly segments identifiers, numbers, punctuation, and whitespace."""
    try:
        import tiktoken
        enc = tiktoken.get_encoding("cl100k_base")
        return len(enc.encode(text))
    except (ImportError, Exception):
        # High-accuracy code/text tokenizer approximation
        # Splits: words/identifiers, numbers, individual punctuation chars, whitespace spans
        tokens = re.findall(r"[a-zA-Z_]+|[0-9]+|[^\s\w]|\s+", text)
        return max(1, len(tokens))


def run_cmd_single(args: list[str], cwd: str) -> tuple[int, str, str, float, float]:
    """Execute a single command, returning (returncode, stdout, stderr, elapsed_ms, peak_rss_mb)."""
    start = time.perf_counter()
    if hasattr(os, "fork") and hasattr(os, "wait4"):
        r_out, w_out = os.pipe()
        r_err, w_err = os.pipe()
        pid = os.fork()
        if pid == 0:
            try:
                os.close(r_out)
                os.close(r_err)
                os.dup2(w_out, 1)
                os.dup2(w_err, 2)
                os.close(w_out)
                os.close(w_err)
                os.chdir(cwd)
                os.execvp(args[0], args)
            except Exception:
                os._exit(127)
        else:
            os.close(w_out)
            os.close(w_err)
            os.set_blocking(r_out, False)
            os.set_blocking(r_err, False)

            sel = selectors.DefaultSelector()
            sel.register(r_out, selectors.EVENT_READ)
            sel.register(r_err, selectors.EVENT_READ)

            stdout_chunks: list[bytes] = []
            stderr_chunks: list[bytes] = []
            open_fds = {r_out, r_err}

            while open_fds:
                for key, _ in sel.select():
                    fd = key.fileobj
                    chunk = os.read(fd, 65536)
                    if not chunk:
                        sel.unregister(fd)
                        os.close(fd)
                        open_fds.remove(fd)
                    else:
                        if fd == r_out:
                            stdout_chunks.append(chunk)
                        else:
                            stderr_chunks.append(chunk)
            sel.close()

            _, status, rusage = os.wait4(pid, 0)
            elapsed_ms = (time.perf_counter() - start) * 1000.0
            stdout = b"".join(stdout_chunks).decode("utf-8", errors="replace")
            stderr = b"".join(stderr_chunks).decode("utf-8", errors="replace")
            rc = os.waitstatus_to_exitcode(status) if hasattr(os, "waitstatus_to_exitcode") else os.WEXITSTATUS(status)
            rss_factor = 1024.0 * 1024.0 if sys.platform == "darwin" else 1024.0
            rss_mb = rusage.ru_maxrss / rss_factor
            return rc, stdout, stderr, elapsed_ms, rss_mb
    else:
        proc = subprocess.run(
            args,
            cwd=cwd,
            capture_output=True,
            text=True,
        )
        elapsed_ms = (time.perf_counter() - start) * 1000.0
        return proc.returncode, proc.stdout, proc.stderr, elapsed_ms, 0.0


def measure_memory_and_latency(binary: str, args: list[str], cwd: str, iterations: int = 5) -> dict[str, Any]:
    latencies: list[float] = []
    rss_list: list[float] = []
    output_text = ""

    for i in range(iterations):
        rc, stdout, stderr, ms, rss = run_cmd_single([binary] + args, cwd=cwd)
        if rc != 0:
            raise RuntimeError(f"Command failed: {args}\n{stderr}")
        latencies.append(ms)
        if rss > 0:
            rss_list.append(rss)
        output_text = stdout

    cold_ms = latencies[0]
    warm_latencies = latencies[1:] if len(latencies) > 1 else latencies
    peak_rss = max(rss_list) if rss_list else 0.0

    return {
        "cold_ms": cold_ms,
        "min_ms": min(warm_latencies),
        "median_ms": statistics.median(warm_latencies),
        "mean_ms": statistics.mean(warm_latencies),
        "p95_ms": percentile(warm_latencies, 0.95),
        "max_ms": max(warm_latencies),
        "raw_samples_ms": latencies,
        "warm_samples_ms": warm_latencies,
        "peak_rss_mb": peak_rss,
        "output": output_text,
        "tokens": estimate_tokens(output_text),
        "bytes": len(output_text.encode("utf-8")),
    }


class PersistentMcpClient:
    """Manages a live stdio JSON-RPC MCP server session."""

    def __init__(self, binary: str, cwd: str):
        self.cwd = cwd
        self.temp_telemetry_dir = tempfile.mkdtemp(prefix="code_kb_telemetry_bench_")
        env = {**os.environ, "CODE_KB_TELEMETRY_DIR": self.temp_telemetry_dir}
        self.proc = subprocess.Popen(
            [binary, "serve"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            cwd=cwd,
            env=env,
        )
        self.next_id = 1

    def send(self, obj: dict[str, Any]) -> None:
        line = json.dumps(obj) + "\n"
        self.proc.stdin.write(line)
        self.proc.stdin.flush()

    def recv(self) -> dict[str, Any]:
        line = self.proc.stdout.readline()
        if not line:
            stderr = self.proc.stderr.read() if self.proc.stderr else ""
            raise RuntimeError(f"Server closed connection unexpectedly. Stderr: {stderr}")
        return json.loads(line)

    def initialize(self) -> float:
        start = time.perf_counter()
        req_id = self.next_id
        self.next_id += 1
        self.send({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "benchmark_quality", "version": "1.0"},
            },
        })
        resp = self.recv()
        elapsed_ms = (time.perf_counter() - start) * 1000.0
        mcp_result(resp, req_id, "Initialization")
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return elapsed_ms

    def call_tool(self, name: str, arguments: dict[str, Any]) -> tuple[dict[str, Any], float]:
        start = time.perf_counter()
        req_id = self.next_id
        self.next_id += 1
        self.send({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        resp = self.recv()
        elapsed_ms = (time.perf_counter() - start) * 1000.0
        mcp_result(resp, req_id, f"Tool call {name}")
        return resp, elapsed_ms

    def get_live_memory(self) -> dict[str, float]:
        pid = self.proc.pid
        try:
            smaps_path = f"/proc/{pid}/smaps_rollup"
            if os.path.exists(smaps_path):
                with open(smaps_path, "r", encoding="utf-8") as f:
                    smaps = f.read()
                rss_m = re.search(r"Rss:\s+(\d+)\s+kB", smaps)
                pss_m = re.search(r"Pss:\s+(\d+)\s+kB", smaps)
                anon_m = re.search(r"Anonymous:\s+(\d+)\s+kB", smaps)
                return {
                    "rss_mb": round(float(rss_m.group(1)) / 1024.0, 2) if rss_m else 0.0,
                    "pss_mb": round(float(pss_m.group(1)) / 1024.0, 2) if pss_m else 0.0,
                    "anon_mb": round(float(anon_m.group(1)) / 1024.0, 2) if anon_m else 0.0,
                }
            statm_path = f"/proc/{pid}/statm"
            if os.path.exists(statm_path):
                with open(statm_path, "r", encoding="utf-8") as f:
                    parts = f.read().split()
                pagesize = 4096.0
                rss_bytes = int(parts[1]) * pagesize
                return {"rss_mb": round(rss_bytes / (1024.0 * 1024.0), 2), "pss_mb": 0.0, "anon_mb": 0.0}
        except Exception:
            pass
        return {"rss_mb": 0.0, "pss_mb": 0.0, "anon_mb": 0.0}

    def close(self) -> None:
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
            self.proc.terminate()
            self.proc.wait(timeout=2)
        except Exception:
            try:
                self.proc.kill()
            except Exception:
                pass
        finally:
            if hasattr(self, "temp_telemetry_dir") and os.path.exists(self.temp_telemetry_dir):
                shutil.rmtree(self.temp_telemetry_dir, ignore_errors=True)


def measure_persistent_mcp(binary: str, cwd: str, iterations: int = 5) -> dict[str, Any]:
    client = PersistentMcpClient(binary, cwd)
    try:
        init_ms = client.initialize()
        baseline_mem = client.get_live_memory()

        # Warm-up call
        client.call_tool("lookup_symbol", {"query": "search_symbols_scoped"})

        tool_specs = [
            ("codebase_outline", {"depth": 2}),
            ("file_skeleton", {"file_path": "crates/code-kb-core/src/queries.rs"}),
            ("lookup_symbol", {"query": "search_symbols_scoped"}),
            ("search_symbols", {"query": "syntax validation"}),
            ("get_symbol_body", {"symbol_name": "validate_syntax"}),
            ("get_symbol_context", {"symbol_name": "search_symbols_scoped"}),
            ("find_references", {"symbol_name": "search_symbols_scoped"}),
            ("blast_radius", {"symbol": "find_callee_signatures"}),
            ("telemetry_summary", {}),
        ]

        tool_results = []
        for name, args in tool_specs:
            _, discarded_warmup_ms = client.call_tool(name, args)
            latencies = []
            sample_tokens = 0
            for _ in range(iterations):
                resp, ms = client.call_tool(name, args)
                latencies.append(ms)
                result = resp["result"]
                if "content" in result:
                    text = "".join(c.get("text", "") for c in result["content"])
                    sample_tokens = estimate_tokens(text)

            tool_results.append({
                "tool": name,
                "args": args,
                "tokens": sample_tokens,
                "discarded_warmup_ms": round(discarded_warmup_ms, 2),
                "raw_samples_ms": [round(ms, 2) for ms in latencies],
                "min_ms": round(min(latencies), 2),
                "p50_ms": round(percentile(latencies, 0.50), 2),
                "median_ms": round(statistics.median(latencies), 2),
                "mean_ms": round(statistics.mean(latencies), 2),
                "p95_ms": round(percentile(latencies, 0.95), 2),
                "max_ms": round(max(latencies), 2),
            })

        post_query_mem = client.get_live_memory()

        # Changed-file reconciliation benchmark
        probe_id = uuid.uuid4().hex[:8]
        probe_symbol = f"probe_benchmark_sym_{probe_id}"
        probe_path = os.path.join(cwd, f"reconcile_benchmark_probe_{probe_id}.rs")
        reconcile_ms = None
        reconcile_found = False
        try:
            with open(probe_path, "w", encoding="utf-8") as f:
                f.write(f"pub fn {probe_symbol}() -> usize {{ 42 }}\n")

            start_reconcile = time.perf_counter()
            deadline = start_reconcile + 2.0
            while time.perf_counter() < deadline:
                resp, _ = client.call_tool("lookup_symbol", {"query": probe_symbol})
                if contains_symbol_declaration(resp["result"], probe_symbol, Path(probe_path).name):
                    reconcile_found = True
                    reconcile_ms = (time.perf_counter() - start_reconcile) * 1000.0
                    break
                time.sleep(0.05)
        except OSError:
            pass
        finally:
            if os.path.exists(probe_path):
                try:
                    os.remove(probe_path)
                except OSError:
                    pass
            # Reconcile removal
            client.call_tool("lookup_symbol", {"query": "search_symbols_scoped"})

        final_mem = client.get_live_memory()

        return {
            "init_handshake_ms": round(init_ms, 2),
            "baseline_memory": baseline_mem,
            "post_query_memory": post_query_mem,
            "final_memory": final_mem,
            "tool_calls": tool_results,
            "reconcile_benchmark": {
                "elapsed_ms": round(reconcile_ms, 2) if reconcile_ms is not None else None,
                "reconciled_symbol_found": reconcile_found,
            },
        }
    finally:
        client.close()


def measure_large_corpus(binary: str, num_files: int = 500, iterations: int = 3) -> dict[str, Any]:
    """Opt-in benchmark generating a disposable large synthetic project and measuring indexing & queries."""
    temp_dir = tempfile.mkdtemp(prefix="code_kb_large_corpus_")
    try:
        # Generate synthetic repository
        subprocess.run(["git", "init", "-q"], cwd=temp_dir, check=True)
        subprocess.run(["git", "config", "user.name", "bench"], cwd=temp_dir, check=True)
        subprocess.run(["git", "config", "user.email", "bench@test.com"], cwd=temp_dir, check=True)

        src_dir = os.path.join(temp_dir, "src")
        os.makedirs(src_dir, exist_ok=True)

        for i in range(num_files):
            sub = os.path.join(src_dir, f"module_{i // 50}")
            os.makedirs(sub, exist_ok=True)
            file_path = os.path.join(sub, f"worker_{i}.rs")
            with open(file_path, "w", encoding="utf-8") as f:
                f.write(f"""//! Synthetic module {i} for large corpus benchmark.

pub struct Worker{i} {{
    pub id: usize,
}}

impl Worker{i} {{
    pub fn new(id: usize) -> Self {{
        Self {{ id }}
    }}

    pub fn process_{i}(&self) -> usize {{
        self.id * 2
    }}
}}
""")

        # Measure scan duration and artifact size
        rc, _, stderr, scan_ms, scan_rss = run_cmd_single([binary, "scan"], cwd=temp_dir)
        if rc != 0:
            raise RuntimeError(f"Scan failed on large corpus: {stderr}")

        db_path = os.path.join(temp_dir, ".code-kb", "artifact.db")
        db_size_mb = os.path.getsize(db_path) / (1024.0 * 1024.0) if os.path.exists(db_path) else 0.0

        # Query latency on large corpus
        lookup_res = measure_memory_and_latency(binary, ["symbol", "process_100"], cwd=temp_dir, iterations=iterations)
        search_res = measure_memory_and_latency(binary, ["search", "synthetic worker"], cwd=temp_dir, iterations=iterations)

        return {
            "num_files": num_files,
            "scan_ms": round(scan_ms, 2),
            "scan_peak_rss_mb": round(scan_rss, 2),
            "artifact_db_size_mb": round(db_size_mb, 2),
            "lookup_process_100_median_ms": lookup_res["median_ms"],
            "search_synthetic_worker_median_ms": search_res["median_ms"],
        }
    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser(description="code-kb Performance & Quality Benchmark")
    parser.add_argument(
        "--binary",
        default=str(Path(__file__).resolve().parents[1] / "target/release/code-kb"),
        help="Path to code-kb binary",
    )
    parser.add_argument(
        "--cwd",
        default=str(Path(__file__).resolve().parents[1]),
        help="Workspace root",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=5,
        help="Number of iterations per query",
    )
    parser.add_argument(
        "--json-output",
        default="benchmark_report.json",
        help="Path to write JSON benchmark report",
    )
    parser.add_argument(
        "--skip-cli",
        action="store_true",
        help="Skip fresh CLI invocation benchmarks",
    )
    parser.add_argument(
        "--skip-mcp",
        action="store_true",
        help="Skip persistent MCP session benchmark",
    )
    parser.add_argument(
        "--large-corpus",
        type=int,
        default=None,
        metavar="N_FILES",
        help="Opt-in disposable large synthetic corpus benchmark with N files",
    )
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error("--iterations must be at least 1")

    binary = str(Path(args.binary).resolve())
    if not os.path.isfile(binary):
        print(f"Binary not found at {binary}. Building release binary...")
        subprocess.run(["cargo", "build", "--release"], cwd=args.cwd, check=True)

    version_result = subprocess.run([binary, "--version"], cwd=args.cwd, capture_output=True, text=True)
    binary_version = version_result.stdout.strip() if version_result.returncode == 0 else "unknown"
    binary_commit = os.environ.get("CODE_KB_BENCHMARK_BINARY_COMMIT")

    print("=================================================================")
    print("           code-kb Token Efficiency & Quality Benchmark          ")
    print("=================================================================")
    print(f"Binary: {binary}")
    print(f"Binary version: {binary_version}")
    print(f"Workspace: {args.cwd}")
    print(f"Iterations: {args.iterations}")
    print("Each invocation is a fresh CLI process; repeats benefit from filesystem cache. Peak RSS is for an exited process, not retained MCP or process-tree memory.")
    print()

    temp_telemetry_dir = tempfile.mkdtemp(prefix="code_kb_telemetry_bench_")
    os.environ["CODE_KB_TELEMETRY_DIR"] = temp_telemetry_dir
    import atexit
    atexit.register(shutil.rmtree, temp_telemetry_dir, ignore_errors=True)

    # 1. Binary Footprint
    bin_size_mb = os.path.getsize(binary) / (1024 * 1024)
    print(f"Binary Size: {bin_size_mb:.2f} MB\n")

    skeleton_results = []
    slice_results = []
    query_benchmarks = []

    if not args.skip_cli:
            # 2. Token Efficiency Benchmarks (File Skeleton vs Full File)
            target_files = [
                ("crates/code-kb-core/src/queries.rs", "queries.rs"),
                ("crates/code-kb-cli/src/mcp/server.rs", "server.rs"),
                ("crates/code-kb-core/src/workspace.rs", "workspace.rs"),
                ("crates/code-kb-core/src/ops.rs", "ops.rs"),
            ]

            for rel_path, label in target_files:
                full_path = os.path.join(args.cwd, rel_path)
                with open(full_path, "r", encoding="utf-8") as f:
                    raw_content = f.read()
                raw_tokens = estimate_tokens(raw_content)
                raw_lines = len(raw_content.splitlines())

                res = measure_memory_and_latency(binary, ["skeleton", rel_path], args.cwd, args.iterations)
                skel_tokens = res["tokens"]
                reduction_pct = ((raw_tokens - skel_tokens) / raw_tokens) * 100.0

                skeleton_results.append({
                    "file": label,
                    "raw_lines": raw_lines,
                    "raw_tokens": raw_tokens,
                    "skeleton_tokens": skel_tokens,
                    "reduction_pct": reduction_pct,
                    "cold_ms": res["cold_ms"],
                    "median_ms": res["median_ms"],
                    "p95_ms": res["p95_ms"],
                    "raw_samples_ms": res["raw_samples_ms"],
                    "warm_samples_ms": res["warm_samples_ms"],
                    "peak_rss_mb": res["peak_rss_mb"],
                })

            # 3. Surgical Context Slice vs Full File Read
            slice_tests = [
                ("search_symbols_scoped", "crates/code-kb-core/src/queries.rs"),
                ("file_skeleton_op", "crates/code-kb-core/src/ops.rs"),
                ("format_file_skeleton", "crates/code-kb-core/src/formatters.rs"),
            ]
            for symbol, file_path in slice_tests:
                full_path = os.path.join(args.cwd, file_path)
                with open(full_path, "r", encoding="utf-8") as f:
                    raw_file_tokens = estimate_tokens(f.read())
                res = measure_memory_and_latency(binary, ["slice", symbol], args.cwd, args.iterations)
                slice_tokens = res["tokens"]
                saving_pct = ((raw_file_tokens - slice_tokens) / raw_file_tokens) * 100.0

                slice_results.append({
                    "symbol": symbol,
                    "file": file_path,
                    "raw_file_tokens": raw_file_tokens,
                    "slice_tokens": slice_tokens,
                    "saving_pct": saving_pct,
                    "cold_ms": res["cold_ms"],
                    "median_ms": res["median_ms"],
                    "p95_ms": res["p95_ms"],
                    "raw_samples_ms": res["raw_samples_ms"],
                    "warm_samples_ms": res["warm_samples_ms"],
                    "peak_rss_mb": res["peak_rss_mb"],
                })

            # 4. Search Quality & Precision Check (Rank-1 / Top-5 Evaluation)
            search_specs = [
                {
                    "name": "Exact Symbol Lookup",
                    "args": ["--json", "symbol", "search_symbols_scoped"],
                    "target": "search_symbols_scoped",
                    "eval_fn": lambda items, tgt: next((idx + 1 for idx, it in enumerate(items) if it.get("name") == tgt), None),
                },
                {
                    "name": "Prefix Symbol Lookup",
                    "args": ["--json", "symbol", "load_scoped"],
                    "target": "load_scoped_files",
                    "eval_fn": lambda items, tgt: next((idx + 1 for idx, it in enumerate(items) if it.get("name") == tgt), None),
                },
                {
                    "name": "Conceptual FTS5 Search",
                    "args": ["--json", "search", "syntax validation"],
                    "target": "validate_syntax",
                    "eval_fn": lambda items, tgt: next((idx + 1 for idx, it in enumerate(items) if it.get("symbol", {}).get("name") == tgt), None),
                },
                {
                    "name": "Unscoped Symbol Search",
                    "args": ["--json", "symbol", "QueryError"],
                    "target": "QueryError",
                    "eval_fn": lambda items, tgt: next((idx + 1 for idx, it in enumerate(items) if it.get("name") == tgt), None),
                },
                {
                    "name": "Scoped Search (--path)",
                    "args": ["--json", "symbol", "QueryError", "--path", "crates/code-kb-core"],
                    "target": "QueryError",
                    "eval_fn": lambda items, tgt: next((idx + 1 for idx, it in enumerate(items) if it.get("name") == tgt and "crates/code-kb-core" in it.get("path", "")), None),
                },
                {
                    "name": "Blast Radius Test Prediction",
                    "args": ["--json", "blast-radius", "find_callee_signatures"],
                    "target": "test_conservative_pending_resolution_ignores_unmatched_namespace",
                    "eval_fn": lambda obj, tgt: next((idx + 1 for idx, t in enumerate(obj.get("likely_tests", [])) if t.get("name") == tgt), None) if isinstance(obj, dict) else None,
                },
            ]

            for spec in search_specs:
                res = measure_memory_and_latency(binary, spec["args"], args.cwd, args.iterations)
                try:
                    parsed = json.loads(res["output"])
                    rank = spec["eval_fn"](parsed, spec["target"])
                except Exception:
                    rank = None

                rank_1 = (rank == 1)
                top_5 = (rank is not None and rank <= 5)

                query_benchmarks.append({
                    "name": spec["name"],
                    "args": [a for a in spec["args"] if a != "--json"],
                    "target": spec["target"],
                    "rank": rank,
                    "rank_1": rank_1,
                    "top_5": top_5,
                    "tokens": res["tokens"],
                    "cold_ms": res["cold_ms"],
                    "median_ms": res["median_ms"],
                    "p95_ms": res["p95_ms"],
                    "raw_samples_ms": res["raw_samples_ms"],
                    "warm_samples_ms": res["warm_samples_ms"],
                    "peak_rss_mb": res["peak_rss_mb"],
                })

    report: dict[str, Any] = {
        "binary_size_mb": bin_size_mb,
        "binary_version": binary_version,
        "metadata": {
            "binary_path": binary,
            "binary_sha256": file_sha256(binary),
            "binary_commit": binary_commit,
            "workspace": str(Path(args.cwd).resolve()),
            "iterations": args.iterations,
            "workloads": {
                "cli": not args.skip_cli,
                "persistent_mcp": not args.skip_mcp,
                "large_corpus_files": args.large_corpus,
            },
        },
    }

    if not args.skip_cli:
        print("### 1. Token Compression: Skeletons vs Full File Reads")
        print("| File | Raw Lines | Raw Tokens | Skeleton Tokens | Token Savings | First CLI Invocation | Fresh CLI Median | Fresh CLI p95 | Peak Exited RSS |")
        print("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
        for r in skeleton_results:
            print(f"| `{r['file']}` | {r['raw_lines']:,} | ~{r['raw_tokens']:,} | ~{r['skeleton_tokens']:,} | **{r['reduction_pct']:.1f}%** | {r['cold_ms']:.2f} ms | {r['median_ms']:.2f} ms | {r['p95_ms']:.2f} ms | {r['peak_rss_mb']:.1f} MB |")
        print()

        print("### 2. Surgical Context Slicing vs Full File Reads")
        print("| Target Symbol | Raw File Tokens | Slice Tokens | Token Savings | First CLI Invocation | Fresh CLI Median | Fresh CLI p95 | Peak Exited RSS |")
        print("|---|---:|---:|---:|---:|---:|---:|---:|")
        for r in slice_results:
            print(f"| `{r['symbol']}` | ~{r['raw_file_tokens']:,} | ~{r['slice_tokens']:,} | **{r['saving_pct']:.1f}%** | {r['cold_ms']:.2f} ms | {r['median_ms']:.2f} ms | {r['p95_ms']:.2f} ms | {r['peak_rss_mb']:.1f} MB |")
        print()

        print("### 3. Query Latency & Search Quality")
        print("| Query Type | Command | Target Symbol | Exact Rank | Top-1 | Top-5 | First CLI Invocation | Fresh CLI Median | Fresh CLI p95 |")
        print("|---|---|---|:---:|:---:|:---:|---:|---:|---:|")
        for q in query_benchmarks:
            rank_str = f"#{q['rank']}" if q["rank"] else "N/A"
            top1_str = "YES" if q["rank_1"] else "NO"
            top5_str = "YES" if q["top_5"] else "NO"
            print(f"| {q['name']} | `code-kb {' '.join(q['args'])}` | `{q['target']}` | {rank_str} | {top1_str} | {top5_str} | {q['cold_ms']:.2f} ms | {q['median_ms']:.2f} ms | {q['p95_ms']:.2f} ms |")
        print()

        max_rss = max(
            max((r["peak_rss_mb"] for r in skeleton_results), default=0.0),
            max((r["peak_rss_mb"] for r in slice_results), default=0.0),
            max((q["peak_rss_mb"] for q in query_benchmarks), default=0.0),
        )
        median_latencies = [q["median_ms"] for q in query_benchmarks]

        print("### 4. Fresh CLI Invocation Measurement Summary")
        print(f"- **Standalone Binary Size:** {bin_size_mb:.2f} MB")
        print(f"- **Binary Version:** {binary_version}")
        print(f"- **Max Peak Exited-Process RSS:** {max_rss:.2f} MB (includes process startup, dynamic link, and SQLite; excludes retained MCP and launcher process-tree memory)")
        print(f"- **Median Fresh CLI Query Invocation:** {statistics.median(median_latencies):.2f} ms (filesystem-warm)")
        print(f"- **Min Fresh CLI Query Invocation:** {min(median_latencies):.2f} ms")
        print(f"- **Search Top-1 Precision:** {sum(1 for q in query_benchmarks if q['rank_1'])}/{len(query_benchmarks)}")
        print(f"- **Search Top-5 Precision:** {sum(1 for q in query_benchmarks if q['top_5'])}/{len(query_benchmarks)}")
        print()

        report.update({
            "measurement_scope": "fresh CLI invocations; repeats benefit from filesystem cache; exited-process peak RSS only, excluding retained MCP server and launcher process-tree memory",
            "max_peak_rss_mb": max_rss,
            "skeleton_compression": skeleton_results,
            "slice_compression": slice_results,
            "queries": query_benchmarks,
        })

    if not args.skip_mcp:
        print("### 5. Persistent MCP Server Session Benchmark")
        print("Measuring live stdio JSON-RPC MCP server session over persistent connection...")
        mcp_res = measure_persistent_mcp(binary, args.cwd, iterations=args.iterations)
        print(f"- **Initial Handshake (initialize + initialized):** {mcp_res['init_handshake_ms']:.2f} ms")
        b_mem = mcp_res["baseline_memory"]
        f_mem = mcp_res["final_memory"]
        print(f"- **Live Baseline Retained Memory:** RSS: {b_mem['rss_mb']:.2f} MB | PSS: {b_mem['pss_mb']:.2f} MB | Anon: {b_mem['anon_mb']:.2f} MB")
        print(f"- **Live Post-Workload Retained Memory:** RSS: {f_mem['rss_mb']:.2f} MB | PSS: {f_mem['pss_mb']:.2f} MB | Anon: {f_mem['anon_mb']:.2f} MB")
        recon = mcp_res["reconcile_benchmark"]
        if recon.get("elapsed_ms") is not None:
            print(f"- **Changed-File Reconciliation & Live Extraction:** {recon['elapsed_ms']:.2f} ms (symbol found: {recon['reconciled_symbol_found']})")
        print()
        print("| Tool | Call Arguments | Tokens | Discarded Warm-up | Min Latency | p50 Latency | Median | Mean | p95 Latency | Max Latency |")
        print("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|")
        for tc in mcp_res["tool_calls"]:
            args_str = json.dumps(tc["args"])
            if len(args_str) > 35:
                args_str = args_str[:32] + "..."
            print(f"| `{tc['tool']}` | `{args_str}` | ~{tc['tokens']:,} | {tc['discarded_warmup_ms']:.2f} ms | {tc['min_ms']:.2f} ms | {tc['p50_ms']:.2f} ms | {tc['median_ms']:.2f} ms | {tc['mean_ms']:.2f} ms | {tc['p95_ms']:.2f} ms | {tc['max_ms']:.2f} ms |")
        print()
        report["persistent_mcp"] = mcp_res

    if args.large_corpus is not None:
        print(f"### 6. Large Disposable Synthetic Corpus Benchmark ({args.large_corpus} files)")
        print(f"Generating disposable {args.large_corpus}-file project and measuring scan & queries...")
        lc_res = measure_large_corpus(binary, num_files=args.large_corpus, iterations=args.iterations)
        print(f"- **Scan Time ({lc_res['num_files']} files):** {lc_res['scan_ms']:.2f} ms ({lc_res['scan_ms']/1000.0:.2f} s)")
        print(f"- **Scan Peak RSS:** {lc_res['scan_peak_rss_mb']:.2f} MB")
        print(f"- **Generated Artifact DB Size:** {lc_res['artifact_db_size_mb']:.2f} MB")
        print(f"- **Lookup Symbol Latency (median):** {lc_res['lookup_process_100_median_ms']:.2f} ms")
        print(f"- **FTS Search Latency (median):** {lc_res['search_synthetic_worker_median_ms']:.2f} ms")
        print()
        report["large_corpus"] = lc_res

    report_path = Path(args.cwd) / args.json_output
    with open(report_path, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2)
    print(f"Report written to {report_path}")


if __name__ == "__main__":
    main()
