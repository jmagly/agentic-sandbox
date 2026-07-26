#!/usr/bin/env python3

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/observability/run-capacity-baseline.py"
SPEC = importlib.util.spec_from_file_location("capacity_baseline", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def valid_config():
    return {
        "schema": MODULE.SCHEMA,
        "duration_seconds": 604800,
        "interval_seconds": 60,
        "request_timeout_seconds": 5,
        "management_url": "http://127.0.0.1:8122",
        "prometheus_url": "http://127.0.0.1:9090",
        "agents": [
            {"id": "capacity-host", "runtime": "host"},
            {"id": "capacity-docker", "runtime": "docker"},
            {"id": "capacity-qemu", "runtime": "qemu"},
        ],
    }


class CapacityBaselineTest(unittest.TestCase):
    def test_committed_config_is_valid(self):
        config = MODULE.load_json(
            ROOT / "configs/observability-capacity-baseline.json"
        )
        MODULE.validate_config(config)

    def test_shortened_window_is_rejected(self):
        config = valid_config()
        config["duration_seconds"] = 604799
        with self.assertRaisesRegex(ValueError, "seven days"):
            MODULE.validate_config(config)

    def test_runtime_mix_is_exact(self):
        config = valid_config()
        config["agents"].pop()
        with self.assertRaisesRegex(ValueError, "runtime mix"):
            MODULE.validate_config(config)

    def test_credentials_in_url_are_rejected(self):
        config = valid_config()
        config["management_url"] = "https://user:secret@example.test"
        with self.assertRaisesRegex(ValueError, "credentials"):
            MODULE.validate_config(config)

    def test_summary_never_contains_event_response_bodies(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / "manifest.json").write_text(
                json.dumps(
                    {
                        "schema": MODULE.SCHEMA,
                        "completed_at": "2026-07-08T00:00:00Z",
                        "actual_duration_seconds": 604800,
                    }
                )
            )
            events = [
                MODULE.event(
                    "task",
                    runtime="host",
                    status=200,
                    latency_ms=10,
                    outcome="success",
                    state="completed",
                ),
                MODULE.event(
                    "task",
                    runtime="host",
                    status=500,
                    latency_ms=30,
                    outcome="failed",
                    state="failed",
                ),
                MODULE.event(
                    "prometheus:storage",
                    status=200,
                    latency_ms=5,
                    outcome="success",
                    value=1024,
                ),
            ]
            MODULE.append_events(output / "events.jsonl", events)
            MODULE.summarize(output)
            summary = MODULE.load_json(output / "summary.json")
            self.assertTrue(summary["complete_seven_day_window"])
            task = next(
                row
                for row in summary["series"]
                if row["operation"] == "task"
            )
            self.assertEqual(task["samples"], 2)
            self.assertEqual(task["success_rate"], 0.5)
            self.assertEqual(task["latency_ms"]["p95"], 30)
            serialized = json.dumps(summary)
            self.assertNotIn("response_body", serialized)
            self.assertNotIn("secret", serialized)


if __name__ == "__main__":
    unittest.main()
