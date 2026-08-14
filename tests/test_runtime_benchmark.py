#!/usr/bin/env python3

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/benchmark-runtimes.py"
SPEC = importlib.util.spec_from_file_location("runtime_benchmark", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def valid_config():
    return {
        "schema": MODULE.SCHEMA,
        "samples": 3,
        "warmups": 0,
        "timeout_seconds": 30,
        "workload": {"cpu_iterations": 100, "io_mib": 1, "task_iterations": 100},
        "runtimes": [
            {"name": "host", "command": ["python3", "-"]},
            {"name": "docker", "command": ["python3", "-"]},
            {"name": "qemu-libvirt", "command": ["python3", "-"]},
            {"name": "cloud-hypervisor", "not_run_reason": "pinned VMM unavailable"},
        ],
    }


class RuntimeBenchmarkTest(unittest.TestCase):
    def test_requires_three_samples(self):
        config = valid_config()
        config["samples"] = 2
        with self.assertRaisesRegex(ValueError, "at least 3"):
            MODULE.validate_config(config)

    def test_requires_complete_runtime_matrix(self):
        config = valid_config()
        config["runtimes"].pop()
        with self.assertRaisesRegex(ValueError, "must define"):
            MODULE.validate_config(config)

    def test_rejects_credential_bearing_adapter(self):
        config = valid_config()
        config["runtimes"][0]["command"].append("Authorization: Bearer value")
        with self.assertRaisesRegex(ValueError, "credential-bearing"):
            MODULE.validate_config(config)

    def test_percentile_uses_nearest_rank(self):
        self.assertEqual(MODULE.percentile([1.0, 2.0, 9.0], 0.95), 9.0)

    def test_runner_writes_raw_csv_summary_and_report(self):
        config = valid_config()
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "evidence"
            MODULE.run(config, output)
            self.assertTrue((output / "raw.json").is_file())
            self.assertTrue((output / "samples.csv").is_file())
            self.assertTrue((output / "REPORT.md").is_file())
            summary = json.loads((output / "summary.json").read_text())
            measured = [row for row in summary["runtimes"] if row["status"] == "measured"]
            self.assertEqual(len(measured), 3)
            self.assertTrue(all(row["samples"] == 3 for row in measured))
            not_run = next(row for row in summary["runtimes"] if row["status"] == "NOT RUN")
            self.assertEqual(not_run["runtime"], "cloud-hypervisor")
            self.assertRegex(summary["benchmark_runner_sha256"], r"^[0-9a-f]{64}$")
            self.assertIsInstance(summary["implementation_worktree_dirty"], bool)
            serialized = json.dumps(summary).lower()
            self.assertNotIn("authorization", serialized)
            self.assertNotIn("hostname", serialized)


if __name__ == "__main__":
    unittest.main()
