#!/usr/bin/env python3

from __future__ import annotations

import copy
import datetime as dt
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(SCRIPT_DIR))
SPEC = importlib.util.spec_from_file_location(
    "activity_validation", SCRIPT_DIR / "activity_validation.py"
)
assert SPEC and SPEC.loader
av = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(av)
oe = av.oe


def validation_config() -> dict:
    return json.loads(
        (SCRIPT_DIR.parents[1] / "configs" / "activity-validation.json").read_text(
            encoding="utf-8"
        )
    )


def evidence_config() -> dict:
    return {
        "schema_version": oe.CONFIG_SCHEMA,
        "management_url": "http://127.0.0.1:8120",
        "prometheus_url": "http://127.0.0.1:9090",
        "sample_interval_seconds": 60,
        "maximum_sample_gap_seconds": 90,
        "request_timeout_seconds": 2,
        "activity_scope": {
            "tenant_id": "tenant-a",
            "host_id": "host-a",
            "instance_id": "instance-a",
            "agent_id": "agent-a",
            "collector_id": "validation-a",
        },
        "identity": {
            "runtime": "host",
            "environment": "test",
            "collector_tier": "activity",
        },
        "sources": [
            {"name": "health", "kind": "health", "path": "/healthz", "required": True}
        ],
        "storage_paths": [],
        "thresholds": {"maximum": {}, "minimum": {}},
        "cost": {"storage_usd_per_gib_month": 0.0, "compute_usd_per_hour": 0.0},
    }


class FakeClient:
    def __init__(self, *, lose_query: bool = False, lose_export: bool = False):
        self.event: dict | None = None
        self.calls: list[str] = []
        self.lose_query = lose_query
        self.lose_export = lose_export

    def ingest(self, event: dict) -> dict:
        self.calls.append("ingest")
        self.event = event
        return {"accepted": 1, "duplicates": 0}

    def query(self) -> dict:
        self.calls.append("query")
        return {"events": [] if self.lose_query else [self.event]}

    def export(self) -> dict:
        self.calls.append("export")
        return {"events": [] if self.lose_export else [self.event]}


class RecordingAdapter:
    def __init__(
        self,
        expected: list[str],
        *,
        metrics: dict[str, float] | None = None,
        rollback: bool = True,
        cleanup: bool = True,
    ):
        self.calls: list[str] = []
        self.expected = expected
        self.metrics = metrics or {"error_count": 0.0, "latency_ms": 1.0}
        self.rollback_result = rollback
        self.cleanup_result = cleanup

    def start(self, profile: dict, target: dict) -> dict:
        self.calls.append("start")
        return {"metrics": self.metrics, "evidence": self.expected}

    def rollback(self, profile: dict, target: dict) -> bool:
        self.calls.append("rollback")
        return self.rollback_result

    def cleanup(self, profile: dict, target: dict) -> bool:
        self.calls.append("cleanup")
        return self.cleanup_result


class ActivityValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.output = Path(self.temp.name)
        self.config = validation_config()
        self.evidence = evidence_config()
        self.now = dt.datetime(2026, 8, 1, 12, 0, tzinfo=dt.timezone.utc)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def samples(self) -> list[dict]:
        return oe.iter_samples(self.output)

    def test_checked_config_defines_only_fixed_safe_profiles(self) -> None:
        av.validate_config(self.config)
        self.assertEqual(set(self.config["profiles"]), av.ALLOWED_EXECUTORS)
        self.assertTrue(all(profile["rollback"] for profile in self.config["profiles"].values()))

    def test_default_deny_rejects_arbitrary_wildcard_unbounded_and_incomplete_profiles(self) -> None:
        mutations = []
        command = copy.deepcopy(self.config)
        command["profiles"]["fixture_backpressure"]["command"] = "anything"
        mutations.append(command)
        wildcard = copy.deepcopy(self.config)
        wildcard["targets"]["bad*"] = wildcard["targets"].pop("local-disposable-fixture")
        mutations.append(wildcard)
        long = copy.deepcopy(self.config)
        long["profiles"]["fixture_backpressure"]["maximum_duration_seconds"] = 301
        mutations.append(long)
        no_rollback = copy.deepcopy(self.config)
        no_rollback["profiles"]["fixture_backpressure"]["rollback"] = ""
        mutations.append(no_rollback)
        no_abort = copy.deepcopy(self.config)
        no_abort["profiles"]["fixture_backpressure"]["abort_thresholds"] = {}
        mutations.append(no_abort)
        undesignated = copy.deepcopy(self.config)
        target = undesignated["targets"]["local-disposable-fixture"]
        target["disposable"] = target["validation_designated"] = False
        mutations.append(undesignated)
        manual_schedule = copy.deepcopy(self.config)
        manual_schedule["schedules"][0]["profile"] = "fixture_evidence_corruption"
        mutations.append(manual_schedule)
        for config in mutations:
            with self.assertRaises(av.ValidationError):
                av.validate_config(config)

    def test_canary_dry_run_is_exact_and_side_effect_free(self) -> None:
        client = FakeClient()
        result = av.run_canary(
            self.config, self.evidence, self.output, client, now=self.now, dry_run=True
        )
        self.assertTrue(result["dry_run"])
        self.assertEqual(result["endpoints"], ["activity-ingest", "activity-query", "activity-export"])
        self.assertEqual(client.calls, [])
        self.assertFalse((self.output / "samples").exists())
        self.assertFalse((self.output / "validation-state.json").exists())

    def test_known_signal_canary_proves_ingest_query_export_and_is_not_organic(self) -> None:
        client = FakeClient()
        result = av.run_canary(self.config, self.evidence, self.output, client, now=self.now)
        self.assertEqual(result["status"], "success")
        self.assertEqual(client.calls, ["ingest", "query", "export"])
        self.assertTrue(all(result["visibility"].values()))
        sample = self.samples()[0]
        self.assertEqual(sample["evidence_class"], "canary")
        self.assertEqual(sample["metrics"]["canary_success"], 1.0)
        state = av.load_state(self.output)
        self.assertEqual(state["outstanding_canaries"], [])

    def test_canary_loss_rate_cap_and_maximum_outstanding_are_enforced(self) -> None:
        config = copy.deepcopy(self.config)
        config["canary"]["deadline_seconds"] = 2000
        config["canary"]["maximum_outstanding"] = 1
        first = av.run_canary(
            config, self.evidence, self.output, FakeClient(lose_query=True), now=self.now
        )
        self.assertEqual(first["status"], "failure")
        with self.assertRaisesRegex(av.ValidationError, "rate budget"):
            av.run_canary(config, self.evidence, self.output, FakeClient(), now=self.now)
        later = self.now + dt.timedelta(seconds=config["canary"]["minimum_interval_seconds"])
        with self.assertRaisesRegex(av.ValidationError, "maximum outstanding"):
            av.run_canary(config, self.evidence, self.output, FakeClient(), now=later)

    def test_every_named_fixture_drill_rolls_back_cleans_and_stays_synthetic(self) -> None:
        for profile in sorted(av.ALLOWED_EXECUTORS):
            result = av.run_drill(
                self.config,
                self.evidence,
                self.output,
                profile,
                "local-disposable-fixture",
                now=self.now,
            )
            self.assertEqual(result["status"], "success")
            self.assertTrue(result["rollback_ok"] and result["cleanup_ok"])
        samples = self.samples()
        self.assertTrue(samples)
        self.assertEqual({sample["evidence_class"] for sample in samples}, {"drill"})
        report = oe.evaluate_window(samples, self.evidence, self.now, 3600)
        self.assertEqual(report["status"], "insufficient_evidence")
        self.assertEqual(report["observed_samples"], 0)

    def test_threshold_abort_still_rolls_back_and_cleans(self) -> None:
        profile = self.config["profiles"]["fixture_backpressure"]
        adapter = RecordingAdapter(
            profile["expected_evidence"], metrics={"queue_depth": 101.0, "latency_ms": 1.0}
        )
        result = av.run_drill(
            self.config,
            self.evidence,
            self.output,
            "fixture_backpressure",
            "local-disposable-fixture",
            adapter=adapter,
            now=self.now,
        )
        self.assertEqual(result["status"], "aborted")
        self.assertEqual(adapter.calls, ["start", "rollback", "cleanup"])
        self.assertTrue(any("queue_depth" in reason for reason in result["abort_reasons"]))
        phases = [sample["validation"]["phase"] for sample in self.samples()]
        self.assertEqual(phases, ["start", "abort", "rollback", "cleanup", "complete"])

    def test_duration_bound_aborts_and_recovers(self) -> None:
        profile = self.config["profiles"]["fixture_collector_restart"]
        adapter = RecordingAdapter(profile["expected_evidence"])
        clock = iter([0.0, 31.0])
        result = av.run_drill(
            self.config,
            self.evidence,
            self.output,
            "fixture_collector_restart",
            "local-disposable-fixture",
            adapter=adapter,
            now=self.now,
            monotonic=lambda: next(clock),
        )
        self.assertEqual(result["status"], "aborted")
        self.assertIn("maximum duration exceeded", result["abort_reasons"])
        self.assertTrue(result["rollback_ok"] and result["cleanup_ok"])

    def test_rollback_and_cleanup_failures_are_explicit(self) -> None:
        profile = self.config["profiles"]["fixture_exporter_outage"]
        rollback = av.run_drill(
            self.config,
            self.evidence,
            self.output,
            "fixture_exporter_outage",
            "local-disposable-fixture",
            adapter=RecordingAdapter(profile["expected_evidence"], rollback=False),
            now=self.now,
        )
        self.assertEqual(rollback["status"], "rollback_failed")
        cleanup = av.run_drill(
            self.config,
            self.evidence,
            self.output,
            "fixture_exporter_outage",
            "local-disposable-fixture",
            adapter=RecordingAdapter(profile["expected_evidence"], cleanup=False),
            now=self.now + dt.timedelta(seconds=1),
        )
        self.assertEqual(cleanup["status"], "cleanup_failed")

    def test_drill_dry_run_does_not_call_adapter_or_write_ledger(self) -> None:
        profile = self.config["profiles"]["fixture_collector_restart"]
        adapter = RecordingAdapter(profile["expected_evidence"])
        result = av.run_drill(
            self.config,
            self.evidence,
            self.output,
            "fixture_collector_restart",
            "local-disposable-fixture",
            adapter=adapter,
            dry_run=True,
            now=self.now,
        )
        self.assertTrue(result["dry_run"])
        self.assertEqual(adapter.calls, [])
        self.assertFalse((self.output / "samples").exists())

    def test_scheduler_runs_due_profile_once_without_idling(self) -> None:
        first = av.run_schedule_once(self.config, self.evidence, self.output, now=self.now)
        self.assertEqual(len(first["runs"]), 1)
        second = av.run_schedule_once(
            self.config, self.evidence, self.output, now=self.now + dt.timedelta(seconds=1)
        )
        self.assertEqual(second["runs"], [])
        later = av.run_schedule_once(
            self.config,
            self.evidence,
            self.output,
            now=self.now + dt.timedelta(days=1),
            dry_run=True,
        )
        self.assertEqual(len(later["runs"]), 1)
        state = av.load_state(self.output)
        self.assertEqual(
            state["schedule_last_run"]["daily-collector-restart-fixture"],
            oe.format_time(self.now),
        )


if __name__ == "__main__":
    unittest.main()
