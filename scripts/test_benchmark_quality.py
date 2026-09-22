import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


MODULE_PATH = Path(__file__).with_name("benchmark_quality.py")
SPEC = importlib.util.spec_from_file_location("benchmark_quality", MODULE_PATH)
benchmark_quality = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark_quality)


class BenchmarkQualityTest(unittest.TestCase):
    def test_mcp_result_rejects_error_and_mismatched_id(self):
        with self.assertRaisesRegex(RuntimeError, "did not match"):
            benchmark_quality.mcp_result({"id": 2, "result": {}}, 1, "tool")
        with self.assertRaisesRegex(RuntimeError, "tool error"):
            benchmark_quality.mcp_result({"id": 1, "result": {"isError": True}}, 1, "tool")
        with self.assertRaisesRegex(RuntimeError, "failed"):
            benchmark_quality.mcp_result({"id": 1, "error": {"message": "bad"}}, 1, "tool")

    def test_probe_requires_exact_symbol_declaration_and_path(self):
        symbol = "codekb_benchmark_absent_probe"
        fallback = {
            "content": [{"text": f"No exact name match; 20 full-text matches for query `{symbol}`."}]
        }
        declaration = {
            "content": [{"text": f"- function `{symbol}` [reconcile_benchmark_probe.rs:1-1]"}]
        }
        self.assertFalse(benchmark_quality.contains_symbol_declaration(fallback, symbol, "reconcile_benchmark_probe.rs"))
        self.assertFalse(benchmark_quality.contains_symbol_declaration(declaration, symbol, "other.rs"))
        self.assertTrue(benchmark_quality.contains_symbol_declaration(declaration, symbol, "reconcile_benchmark_probe.rs"))

    def test_cli_latency_records_warm_samples_and_p95(self):
        samples = iter([
            (0, "ok", "", 100.0, 1.0),
            (0, "ok", "", 4.0, 1.0),
            (0, "ok", "", 8.0, 1.0),
            (0, "ok", "", 12.0, 1.0),
        ])
        with patch.object(benchmark_quality, "run_cmd_single", side_effect=samples):
            result = benchmark_quality.measure_memory_and_latency("code-kb", ["symbol", "name"], ".", iterations=4)
        self.assertEqual(result["cold_ms"], 100.0)
        self.assertEqual(result["warm_samples_ms"], [4.0, 8.0, 12.0])
        self.assertEqual(result["p95_ms"], 12.0)

    def test_main_writes_json_without_large_corpus(self):
        binary = sys.executable
        with tempfile.TemporaryDirectory() as temp_dir:
            with patch.object(sys, "argv", [
                "benchmark_quality.py",
                "--binary", binary,
                "--cwd", temp_dir,
                "--skip-cli",
                "--skip-mcp",
                "--json-output", "report.json",
            ]), contextlib.redirect_stdout(io.StringIO()):
                benchmark_quality.main()
            report = json.loads(Path(temp_dir, "report.json").read_text(encoding="utf-8"))
        self.assertIn("metadata", report)
        self.assertFalse(report["metadata"]["workloads"]["cli"])
        self.assertFalse(report["metadata"]["workloads"]["persistent_mcp"])


if __name__ == "__main__":
    unittest.main()
