#!/usr/bin/env python3

import copy
import importlib.util
import json
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("activity_timeline_poc.py")
SPEC = importlib.util.spec_from_file_location("activity_timeline_poc", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(MODULE)


class ActivityTimelinePocTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fixture = Path(__file__).with_name("fixtures") / "activity-events.jsonl"
        cls.events = MODULE.load_events([fixture])

    def test_fixture_covers_required_planes_and_correlation(self):
        timeline = MODULE.build_timeline(self.events)
        self.assertEqual(timeline["summary"]["event_count"], 7)
        self.assertGreaterEqual(timeline["summary"]["plane_counts"]["session"], 1)
        self.assertGreaterEqual(timeline["summary"]["plane_counts"]["action"], 1)
        self.assertGreaterEqual(timeline["summary"]["plane_counts"]["network"], 1)
        self.assertGreaterEqual(timeline["summary"]["plane_counts"]["runtime"], 1)
        self.assertEqual(timeline["summary"]["instance_count"], 1)
        self.assertEqual(timeline["summary"]["session_count"], 1)

    def test_sensitive_content_is_redacted(self):
        timeline = MODULE.build_timeline(self.events)
        encoded = json.dumps(timeline)
        self.assertNotIn("sensitive provider content", encoded)
        tool_event = next(
            event for event in timeline["events"] if event["event_name"] == "agent.tool.invoked"
        )
        self.assertEqual(tool_event["payload"]["prompt"], MODULE.REDACTED)

    def test_sequence_gap_and_explicit_loss_are_reported(self):
        timeline = MODULE.build_timeline(self.events)
        self.assertFalse(timeline["loss"]["complete"])
        self.assertEqual(timeline["summary"]["sequence_gap_count"], 1)
        self.assertEqual(timeline["summary"]["sequence_missing_events"], 1)
        self.assertEqual(timeline["summary"]["explicit_dropped_events"], 1)
        self.assertEqual(
            timeline["loss"]["sequence_gaps"][0]["collector"], "guest-exec-observer"
        )

    def test_timeline_is_sorted_and_hash_linked(self):
        timeline = MODULE.build_timeline(reversed(self.events))
        times = [event["occurred_at"] for event in timeline["events"]]
        self.assertEqual(times, sorted(times))
        previous = "0" * 64
        for event in timeline["events"]:
            self.assertEqual(event["integrity"]["timeline_previous_hash"], previous)
            previous = event["integrity"]["timeline_hash"]
        self.assertEqual(previous, timeline["summary"]["timeline_chain_head"])

    def test_hash_changes_when_non_redacted_evidence_changes(self):
        baseline = MODULE.build_timeline(self.events)["summary"]["timeline_chain_head"]
        changed = copy.deepcopy(self.events)
        changed[0]["payload"]["session_backend"] = "changed"
        updated = MODULE.build_timeline(changed)["summary"]["timeline_chain_head"]
        self.assertNotEqual(baseline, updated)

    def test_validation_rejects_missing_correlation(self):
        event = copy.deepcopy(self.events[0])
        del event["correlation"]["tenant_id"]
        with self.assertRaises(MODULE.ValidationError):
            MODULE.validate_event(event)

    def test_markdown_includes_loss_and_trust(self):
        rendered = MODULE.render_markdown(MODULE.build_timeline(self.events))
        self.assertIn("## Loss report", rendered)
        self.assertIn("self-reported", rendered)
        self.assertIn("missing 1 event", rendered)

    def test_small_benchmark_reports_resource_and_storage_measures(self):
        report = MODULE.benchmark([20], repetitions=1)
        case = report["cases"][0]
        self.assertEqual(case["events"], 20)
        self.assertGreater(case["events_per_second_p50"], 0)
        self.assertGreater(case["peak_heap_bytes_max"], 0)
        self.assertGreater(case["input_bytes"], 0)
        self.assertGreater(case["input_bytes_per_event"], 0)
        self.assertGreater(case["output_bytes_per_event"], 0)
        self.assertGreater(case["serialized_io_expansion_ratio"], 0)


if __name__ == "__main__":
    unittest.main()
