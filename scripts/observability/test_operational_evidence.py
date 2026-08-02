#!/usr/bin/env python3

from __future__ import annotations

import datetime as dt
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

MODULE_PATH = Path(__file__).with_name("operational_evidence.py")
SPEC = importlib.util.spec_from_file_location("operational_evidence", MODULE_PATH)
assert SPEC and SPEC.loader
oe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(oe)


def config() -> dict:
    return {
        "schema_version": oe.CONFIG_SCHEMA,
        "management_url": "http://127.0.0.1:8120",
        "prometheus_url": "http://127.0.0.1:9090",
        "sample_interval_seconds": 3600,
        "maximum_sample_gap_seconds": 4000,
        "request_timeout_seconds": 2,
        "activity_scope": {
            "tenant_id": "tenant-a",
            "host_id": "host-a",
            "instance_id": "instance-a",
            "agent_id": "agent-a",
            "collector_id": "collector-a",
        },
        "identity": {
            "runtime": "host",
            "environment": "test",
            "collector_tier": "activity",
        },
        "sources": [
            {
                "name": "health",
                "kind": "health",
                "path": "/healthz",
                "required": True,
            },
            {
                "name": "coverage",
                "kind": "activity_coverage",
                "path": "/api/v2/activity/coverage",
                "required": True,
            },
        ],
        "storage_paths": [],
        "thresholds": {
            "maximum": {
                "sequence_gap_count": 0,
                "durable_loss_count": 0,
                "source_latency_ms": 1000,
            },
            "minimum": {"coverage_complete": 1},
        },
        "cost": {
            "storage_usd_per_gib_month": 0.1,
            "compute_usd_per_hour": 0.0,
        },
    }


def sample(
    when: dt.datetime,
    *,
    source_state: str = "available",
    evidence_class: str = "organic",
    evidence_origin: str = "operational",
    coverage_complete: float = 1.0,
    identity_suffix: str = "a",
    monotonic_seconds: int | None = None,
) -> dict:
    value = {
        "schema_version": oe.SAMPLE_SCHEMA,
        "sample_id": f"sample-{when.timestamp()}-{evidence_class}",
        "recorded_at": oe.format_time(when),
        "monotonic_ns": int(
            (monotonic_seconds if monotonic_seconds is not None else when.timestamp())
            * 1_000_000_000
        ),
        "evidence_class": evidence_class,
        "evidence_origin": evidence_origin,
        "identity": {
            "config_sha256": "a" * 64,
            "implementation_commit": identity_suffix,
            "sampler_version": oe.SAMPLER_VERSION,
            "runtime": "host",
            "environment": "test",
            "collector_tier": "activity",
            "scope": config()["activity_scope"],
        },
        "sources": [
            {
                "name": "coverage",
                "kind": "activity_coverage",
                "required": True,
                "availability": source_state,
                "status": 200,
                "latency_ms": 5.0,
                "metrics": {
                    "coverage_complete": coverage_complete,
                    "sequence_gap_count": 0,
                    "durable_loss_count": 0,
                },
            }
        ],
        "metrics": {
            "management_rss_bytes": 1024,
            "activity_storage_bytes": 2048,
            "evidence_artifact_bytes": 128,
            "estimated_storage_usd_per_month": 0.0,
            "estimated_compute_usd_per_hour": 0.0,
        },
    }
    value["sample_digest"] = oe.digest_value(value)
    return value


class ConfigTests(unittest.TestCase):
    def test_valid_config_and_scope_headers(self) -> None:
        value = config()
        oe.validate_config(value)
        self.assertEqual(oe.scope_headers(value)["x-activity-tenant-id"], "tenant-a")

    def test_credentials_query_and_mutating_source_kind_are_rejected(self) -> None:
        value = config()
        value["management_url"] = "http://user:pass@127.0.0.1:8120?token=x"
        with self.assertRaises(oe.EvidenceError):
            oe.validate_config(value)
        value = config()
        value["sources"][0]["kind"] = "task"
        with self.assertRaises(oe.EvidenceError):
            oe.validate_config(value)


class PassiveCollectionTests(unittest.TestCase):
    def test_collector_uses_only_get_surfaces_and_organic_class(self) -> None:
        value = config()
        requested: list[str] = []

        def fake_get(url: str, timeout: int, headers=None):
            del timeout, headers
            requested.append(url)
            if url.endswith("/api/v2/activity/coverage"):
                return (
                    200,
                    {
                        "completeness": {
                            "complete": True,
                            "sequence_gap_count": 0,
                            "durable_loss_count": 0,
                            "restart_count": 0,
                            "dropped_event_count": 0,
                            "stale_collector_count": 0,
                            "maximum_clock_error_ms": 1,
                        }
                    },
                    2.0,
                    "available",
                )
            return 200, {"status": "ok"}, 1.0, "available"

        with tempfile.TemporaryDirectory() as directory, mock.patch.object(
            oe, "read_json_get", side_effect=fake_get
        ), mock.patch.object(oe, "git_commit", return_value="abc123"):
            created = oe.create_sample(
                value,
                Path(directory),
                now=dt.datetime(2026, 8, 1, tzinfo=dt.timezone.utc),
                monotonic_ns=10,
            )
        self.assertEqual(created["evidence_class"], "organic")
        self.assertEqual(created["evidence_origin"], "operational")
        self.assertEqual(len(requested), 2)
        self.assertTrue(all("/messages" not in url for url in requested))
        self.assertTrue(all("/sessions" not in url for url in requested))
        self.assertTrue(all(not url.endswith(("/start", "/stop", "/destroy")) for url in requested))
        oe.validate_sample(created)

    def test_passive_collector_cannot_create_synthetic_classes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(oe.EvidenceError):
                oe.create_sample(config(), Path(directory), evidence_class="canary")


class LedgerTests(unittest.TestCase):
    def test_daily_records_are_atomic_hash_chained_and_verifiable(self) -> None:
        first = dt.datetime(2026, 7, 30, tzinfo=dt.timezone.utc)
        second = first + dt.timedelta(days=1)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            oe.append_sample(output, sample(first))
            oe.append_sample(output, sample(second))
            record_one = oe.seal_day(output, first.date())
            record_two = oe.seal_day(output, second.date())
            self.assertIsNone(record_one["previous_record_digest"])
            self.assertEqual(
                record_two["previous_record_digest"], record_one["record_digest"]
            )
            result = oe.verify_records(output)
            self.assertEqual(result["record_count"], 2)

    def test_sample_and_record_mutation_are_detected(self) -> None:
        when = dt.datetime(2026, 7, 30, tzinfo=dt.timezone.utc)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            sample_path = oe.append_sample(output, sample(when))
            original = sample_path.read_text(encoding="utf-8")
            sample_path.write_text(original.replace('"status":200', '"status":500'), encoding="utf-8")
            with self.assertRaises(oe.EvidenceError):
                oe.iter_samples(output)
            sample_path.write_text(original, encoding="utf-8")
            oe.seal_day(output, when.date())
            record_path = output / "records" / f"{when.date().isoformat()}.json"
            record = json.loads(record_path.read_text(encoding="utf-8"))
            record["summary"]["sample_count"] = 99
            record_path.write_text(json.dumps(record), encoding="utf-8")
            with self.assertRaises(oe.EvidenceError):
                oe.verify_records(output)


class EvaluationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.end = dt.datetime(2026, 8, 1, 12, tzinfo=dt.timezone.utc)

    def test_three_state_windows_and_actual_elapsed_time(self) -> None:
        value = config()
        samples = [sample(self.end - dt.timedelta(hours=1)), sample(self.end)]
        one_hour = oe.evaluate_window(samples, value, self.end, 3600)
        day = oe.evaluate_window(samples, value, self.end, 86400)
        self.assertEqual(one_hour["status"], "pass")
        self.assertEqual(one_hour["actual_consecutive_seconds"], 3600)
        self.assertEqual(day["status"], "insufficient_evidence")

    def test_seven_day_boundary_requires_full_604800_second_span(self) -> None:
        value = config()
        start = self.end - dt.timedelta(days=7)
        complete = [
            sample(start + dt.timedelta(hours=offset)) for offset in range(169)
        ]
        short = complete[:-1] + [sample(self.end - dt.timedelta(seconds=1))]
        complete_result = oe.evaluate_window(complete, value, self.end, 604800)
        short_result = oe.evaluate_window(short, value, self.end, 604800)
        self.assertEqual(complete_result["status"], "pass")
        self.assertEqual(complete_result["actual_consecutive_seconds"], 604800)
        self.assertEqual(short_result["status"], "insufficient_evidence")

    def test_missing_source_is_insufficient_and_threshold_breach_fails(self) -> None:
        value = config()
        missing = [
            sample(self.end - dt.timedelta(hours=1), source_state="missing"),
            sample(self.end, source_state="missing"),
        ]
        failed = [
            sample(self.end - dt.timedelta(hours=1), coverage_complete=0),
            sample(self.end, coverage_complete=0),
        ]
        self.assertEqual(
            oe.evaluate_window(missing, value, self.end, 3600)["status"],
            "insufficient_evidence",
        )
        result = oe.evaluate_window(failed, value, self.end, 3600)
        self.assertEqual(result["status"], "fail")
        self.assertIn("coverage_complete fell below minimum 1", result["threshold_failures"])

    def test_synthetic_and_fixture_samples_cannot_qualify_duration(self) -> None:
        value = config()
        synthetic = [
            sample(self.end - dt.timedelta(hours=1), evidence_class="canary"),
            sample(self.end, evidence_class="drill"),
        ]
        fixture = [
            sample(self.end - dt.timedelta(hours=1), evidence_origin="fixture"),
            sample(self.end, evidence_origin="fixture"),
        ]
        self.assertEqual(
            oe.evaluate_window(synthetic, value, self.end, 3600)["status"],
            "insufficient_evidence",
        )
        fixture_result = oe.evaluate_window(fixture, value, self.end, 3600)
        self.assertEqual(fixture_result["status"], "insufficient_evidence")
        self.assertTrue(any("fixture" in reason for reason in fixture_result["reasons"]))

    def test_clock_reset_gap_and_identity_change_are_insufficient(self) -> None:
        value = config()
        start = sample(self.end - dt.timedelta(hours=1), monotonic_seconds=10_000)
        reset = sample(self.end, monotonic_seconds=1)
        changed = sample(self.end, identity_suffix="b")
        clock_result = oe.evaluate_window([start, reset], value, self.end, 3600)
        identity_result = oe.evaluate_window(
            [sample(self.end - dt.timedelta(hours=1)), changed], value, self.end, 3600
        )
        self.assertEqual(clock_result["status"], "insufficient_evidence")
        self.assertTrue(
            any(item["type"] == "clock_discontinuity" for item in clock_result["interruptions"])
        )
        self.assertEqual(identity_result["status"], "insufficient_evidence")
        self.assertTrue(any("identity changed" in reason for reason in identity_result["reasons"]))


if __name__ == "__main__":
    unittest.main()
