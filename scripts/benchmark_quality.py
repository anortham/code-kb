#!/usr/bin/env python3
"""
Benchmark suite for code-kb: Token efficiency, query latency, memory footprint, and search quality.
Measures real performance, token reduction, peak RSS, and search precision across code-kb operations.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import selectors
import statistics
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


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
        "max_ms": max(warm_latencies),
        "peak_rss_mb": peak_rss,
        "output": output_text,
        "tokens": estimate_tokens(output_text),
        "bytes": len(output_text.encode("utf-8")),
    }


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
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error("--iterations must be at least 1")

    binary = args.binary
    if not os.path.isfile(binary):
        print(f"Binary not found at {binary}. Building release binary...")
        subprocess.run(["cargo", "build", "--release"], cwd=args.cwd, check=True)

    version_result = subprocess.run([binary, "--version"], cwd=args.cwd, capture_output=True, text=True)
    binary_version = version_result.stdout.strip() if version_result.returncode == 0 else "unknown"

    print("=================================================================")
    print("           code-kb Token Efficiency & Quality Benchmark          ")
    print("=================================================================")
    print(f"Binary: {binary}")
    print(f"Binary version: {binary_version}")
    print(f"Workspace: {args.cwd}")
    print(f"Iterations: {args.iterations}")
    print("Each invocation is a fresh CLI process; repeats benefit from filesystem cache. Peak RSS is for an exited process, not retained MCP or process-tree memory.")
    print()

    # 1. Binary Footprint
    bin_size_mb = os.path.getsize(binary) / (1024 * 1024)
    print(f"Binary Size: {bin_size_mb:.2f} MB\n")

    # 2. Token Efficiency Benchmarks (File Skeleton vs Full File)
    target_files = [
        ("crates/code-kb-core/src/queries.rs", "queries.rs"),
        ("crates/code-kb-cli/src/mcp/server.rs", "server.rs"),
        ("crates/code-kb-core/src/workspace.rs", "workspace.rs"),
        ("crates/code-kb-core/src/ops.rs", "ops.rs"),
    ]

    skeleton_results = []
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
            "peak_rss_mb": res["peak_rss_mb"],
        })

    # 3. Surgical Context Slice vs Full File Read
    slice_tests = [
        ("search_symbols_scoped", "crates/code-kb-core/src/queries.rs"),
        ("file_skeleton_op", "crates/code-kb-core/src/ops.rs"),
        ("format_file_skeleton", "crates/code-kb-core/src/formatters.rs"),
    ]
    slice_results = []
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

    query_benchmarks = []
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
            "peak_rss_mb": res["peak_rss_mb"],
        })

    print("### 1. Token Compression: Skeletons vs Full File Reads")
    print("| File | Raw Lines | Raw Tokens | Skeleton Tokens | Token Savings | First CLI Invocation | Fresh CLI Median | Peak Exited RSS |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|")
    for r in skeleton_results:
        print(f"| `{r['file']}` | {r['raw_lines']:,} | ~{r['raw_tokens']:,} | ~{r['skeleton_tokens']:,} | **{r['reduction_pct']:.1f}%** | {r['cold_ms']:.2f} ms | {r['median_ms']:.2f} ms | {r['peak_rss_mb']:.1f} MB |")
    print()

    print("### 2. Surgical Context Slicing vs Full File Reads")
    print("| Target Symbol | Raw File Tokens | Slice Tokens | Token Savings | First CLI Invocation | Fresh CLI Median | Peak Exited RSS |")
    print("|---|---:|---:|---:|---:|---:|---:|")
    for r in slice_results:
        print(f"| `{r['symbol']}` | ~{r['raw_file_tokens']:,} | ~{r['slice_tokens']:,} | **{r['saving_pct']:.1f}%** | {r['cold_ms']:.2f} ms | {r['median_ms']:.2f} ms | {r['peak_rss_mb']:.1f} MB |")
    print()

    print("### 3. Query Latency & Search Quality")
    print("| Query Type | Command | Target Symbol | Exact Rank | Top-1 | Top-5 | First CLI Invocation | Fresh CLI Median |")
    print("|---|---|---|:---:|:---:|:---:|---:|---:|")
    for q in query_benchmarks:
        rank_str = f"#{q['rank']}" if q["rank"] else "N/A"
        top1_str = "YES" if q["rank_1"] else "NO"
        top5_str = "YES" if q["top_5"] else "NO"
        print(f"| {q['name']} | `code-kb {' '.join(q['args'])}` | `{q['target']}` | {rank_str} | {top1_str} | {top5_str} | {q['cold_ms']:.2f} ms | {q['median_ms']:.2f} ms |")
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

    report = {
        "binary_size_mb": bin_size_mb,
        "binary_version": binary_version,
        "measurement_scope": "fresh CLI invocations; repeats benefit from filesystem cache; exited-process peak RSS only, excluding retained MCP server and launcher process-tree memory",
        "max_peak_rss_mb": max_rss,
        "skeleton_compression": skeleton_results,
        "slice_compression": slice_results,
        "queries": query_benchmarks,
    }

    report_path = Path(args.cwd) / args.json_output
    with open(report_path, "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2)
    print(f"Report written to {report_path}")


if __name__ == "__main__":
    main()
