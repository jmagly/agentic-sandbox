#!/usr/bin/env python3
"""Normalize activity evidence into a correlated, loss-aware timeline.

This is the bounded proof of concept for issue #707.  It deliberately does
not collect privileged host data.  Instead, it exercises the contract that
future guest, runtime, host, and control-plane collectors will write.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import os
import statistics
import sys
import time
import tracemalloc
from collections import Counter
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Iterable


SCHEMA_VERSION = "activity.event/v1"
PLANES = {"session", "action", "network", "runtime", "system", "integrity"}
LAYERS = {"guest", "runtime", "host", "control-plane", "provider"}
TRUST_LEVELS = {"observed", "attested", "self-reported", "derived"}
SENSITIVITY = {"metadata", "restricted-content", "secret-prohibited"}
RETENTION = {"standard", "security", "forensic-hold", "ephemeral"}
REQUIRED_CORRELATION = {"tenant_id", "host_id", "instance_id", "agent_id"}
REDACTED = "[REDACTED]"
SENSITIVE_KEY_FRAGMENTS = {
    "authorization",
    "cookie",
    "credential",
    "password",
    "prompt",
    "secret",
    "token",
}


class ValidationError(ValueError):
    """Raised when an input record does not satisfy the PoC contract."""


def _parse_timestamp(value: str, field: str) -> datetime:
    if not isinstance(value, str):
        raise ValidationError(f"{field} must be an RFC3339 string")
    normalized = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError as error:
        raise ValidationError(f"{field} is not RFC3339: {value}") from error
    if parsed.tzinfo is None:
        raise ValidationError(f"{field} must include a timezone")
    return parsed.astimezone(timezone.utc)


def validate_event(event: dict[str, Any]) -> None:
    required = {
        "schema_version",
        "event_id",
        "event_name",
        "plane",
        "occurred_at",
        "observed_at",
        "source",
        "correlation",
        "sensitivity",
        "retention_class",
        "payload",
        "integrity",
    }
    missing = sorted(required - event.keys())
    if missing:
        raise ValidationError(f"missing required fields: {', '.join(missing)}")
    if event["schema_version"] != SCHEMA_VERSION:
        raise ValidationError(f"unsupported schema_version: {event['schema_version']}")
    if event["plane"] not in PLANES:
        raise ValidationError(f"unknown plane: {event['plane']}")
    if event["sensitivity"] not in SENSITIVITY:
        raise ValidationError(f"unknown sensitivity: {event['sensitivity']}")
    if event["retention_class"] not in RETENTION:
        raise ValidationError(f"unknown retention_class: {event['retention_class']}")
    _parse_timestamp(event["occurred_at"], "occurred_at")
    _parse_timestamp(event["observed_at"], "observed_at")

    source = event["source"]
    if not isinstance(source, dict):
        raise ValidationError("source must be an object")
    for field in ("collector", "layer", "runtime", "trust"):
        if not source.get(field):
            raise ValidationError(f"source.{field} is required")
    if source["layer"] not in LAYERS:
        raise ValidationError(f"unknown source.layer: {source['layer']}")
    if source["trust"] not in TRUST_LEVELS:
        raise ValidationError(f"unknown source.trust: {source['trust']}")

    correlation = event["correlation"]
    if not isinstance(correlation, dict):
        raise ValidationError("correlation must be an object")
    missing_correlation = sorted(
        field for field in REQUIRED_CORRELATION if not correlation.get(field)
    )
    if missing_correlation:
        raise ValidationError(
            "missing correlation fields: " + ", ".join(missing_correlation)
        )

    integrity = event["integrity"]
    sequence = integrity.get("collector_sequence") if isinstance(integrity, dict) else None
    if not isinstance(sequence, int) or sequence < 1:
        raise ValidationError("integrity.collector_sequence must be an integer >= 1")
    if not isinstance(event["payload"], dict):
        raise ValidationError("payload must be an object")


def _is_sensitive_key(key: str) -> bool:
    normalized = key.lower().replace("-", "_")
    return any(fragment in normalized for fragment in SENSITIVE_KEY_FRAGMENTS)


def redact(value: Any) -> Any:
    """Redact known content-bearing fields before persistence or rendering."""
    if isinstance(value, dict):
        return {
            key: REDACTED if _is_sensitive_key(key) else redact(child)
            for key, child in value.items()
        }
    if isinstance(value, list):
        return [redact(child) for child in value]
    return value


def canonical_bytes(value: dict[str, Any]) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def load_events(paths: Iterable[Path]) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    for path in paths:
        with path.open("r", encoding="utf-8") as handle:
            for line_number, line in enumerate(handle, 1):
                if not line.strip():
                    continue
                try:
                    event = json.loads(line)
                except json.JSONDecodeError as error:
                    raise ValidationError(f"{path}:{line_number}: invalid JSON: {error}") from error
                try:
                    validate_event(event)
                except ValidationError as error:
                    raise ValidationError(f"{path}:{line_number}: {error}") from error
                events.append(event)
    return events


def build_timeline(events: Iterable[dict[str, Any]]) -> dict[str, Any]:
    normalized = [redact(copy.deepcopy(event)) for event in events]
    normalized.sort(
        key=lambda event: (
            _parse_timestamp(event["occurred_at"], "occurred_at"),
            event["source"]["collector"],
            event["integrity"]["collector_sequence"],
            event["event_id"],
        )
    )

    last_sequence: dict[str, int] = {}
    sequence_gaps: list[dict[str, Any]] = []
    explicit_loss = 0
    chain_head = "0" * 64
    plane_counts: Counter[str] = Counter()
    trust_counts: Counter[str] = Counter()
    runtime_counts: Counter[str] = Counter()
    sessions: set[str] = set()
    instances: set[str] = set()

    for event in normalized:
        collector = event["source"]["collector"]
        sequence = event["integrity"]["collector_sequence"]
        previous = last_sequence.get(collector)
        if previous is not None and sequence > previous + 1:
            sequence_gaps.append(
                {
                    "collector": collector,
                    "after": previous,
                    "before": sequence,
                    "missing": sequence - previous - 1,
                }
            )
        if previous is None or sequence > previous:
            last_sequence[collector] = sequence

        if event["event_name"] == "telemetry.loss":
            dropped = event["payload"].get("dropped_count", 0)
            if isinstance(dropped, int) and dropped > 0:
                explicit_loss += dropped

        event["integrity"]["timeline_previous_hash"] = chain_head
        material = copy.deepcopy(event)
        material["integrity"].pop("timeline_hash", None)
        chain_head = hashlib.sha256(canonical_bytes(material)).hexdigest()
        event["integrity"]["timeline_hash"] = chain_head

        plane_counts[event["plane"]] += 1
        trust_counts[event["source"]["trust"]] += 1
        runtime_counts[event["source"]["runtime"]] += 1
        correlation = event["correlation"]
        instances.add(correlation["instance_id"])
        if correlation.get("session_id"):
            sessions.add(correlation["session_id"])

    return {
        "schema_version": "activity.timeline/v1",
        "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "summary": {
            "event_count": len(normalized),
            "instance_count": len(instances),
            "session_count": len(sessions),
            "plane_counts": dict(sorted(plane_counts.items())),
            "trust_counts": dict(sorted(trust_counts.items())),
            "runtime_counts": dict(sorted(runtime_counts.items())),
            "sequence_gap_count": len(sequence_gaps),
            "sequence_missing_events": sum(gap["missing"] for gap in sequence_gaps),
            "explicit_dropped_events": explicit_loss,
            "timeline_chain_head": chain_head,
        },
        "loss": {
            "sequence_gaps": sequence_gaps,
            "explicit_dropped_events": explicit_loss,
            "complete": not sequence_gaps and explicit_loss == 0,
        },
        "events": normalized,
    }


def _format_payload(event: dict[str, Any]) -> str:
    payload = event["payload"]
    preferred = (
        "tool_name",
        "executable",
        "destination",
        "protocol",
        "cpu_percent",
        "memory_bytes",
        "exit_code",
        "reason",
    )
    parts = [f"{key}={payload[key]}" for key in preferred if key in payload]
    return ", ".join(parts) if parts else "metadata recorded"


def render_markdown(timeline: dict[str, Any]) -> str:
    summary = timeline["summary"]
    lines = [
        "# Agent activity timeline PoC",
        "",
        f"Generated: {timeline['generated_at']}",
        "",
        "## Summary",
        "",
        f"- Events: {summary['event_count']}",
        f"- Instances: {summary['instance_count']}",
        f"- Sessions: {summary['session_count']}",
        f"- Sequence gaps: {summary['sequence_gap_count']} "
        f"({summary['sequence_missing_events']} inferred missing events)",
        f"- Explicit dropped events: {summary['explicit_dropped_events']}",
        f"- Chain head: `{summary['timeline_chain_head']}`",
        "",
        "## Timeline",
        "",
        "| Time | Plane | Event | Source / trust | Correlation | Evidence |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for event in timeline["events"]:
        source = event["source"]
        correlation = event["correlation"]
        correlation_text = "/".join(
            filter(
                None,
                [
                    correlation.get("instance_id"),
                    correlation.get("session_id"),
                    correlation.get("tool_call_id"),
                ],
            )
        )
        lines.append(
            f"| {event['occurred_at']} | {event['plane']} | {event['event_name']} | "
            f"{source['collector']} / {source['trust']} | {correlation_text} | "
            f"{_format_payload(event)} |"
        )

    lines.extend(["", "## Loss report", ""])
    if timeline["loss"]["complete"]:
        lines.append("No sequence gaps or explicit collector loss were reported.")
    else:
        for gap in timeline["loss"]["sequence_gaps"]:
            lines.append(
                f"- Collector `{gap['collector']}` is missing {gap['missing']} event(s) "
                f"between sequence {gap['after']} and {gap['before']}."
            )
        if timeline["loss"]["explicit_dropped_events"]:
            lines.append(
                "- Collectors explicitly reported "
                f"{timeline['loss']['explicit_dropped_events']} dropped event(s)."
            )

    lines.extend(
        [
            "",
            "## Interpretation",
            "",
            "This output proves correlation and loss accounting for a bounded fixture. "
            "The hash chain detects post-normalization mutation but does not prove that "
            "a source collector was complete or truthful; production collectors need "
            "separate keys or an append-only trusted sink.",
            "",
        ]
    )
    return "\n".join(lines)


def _percentile(values: list[float], percentile: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = max(0, math.ceil(percentile * len(ordered)) - 1)
    return ordered[index]


def _synthetic_events(count: int) -> list[dict[str, Any]]:
    base = datetime(2026, 8, 1, 12, 0, tzinfo=timezone.utc)
    events = []
    names = (
        ("session", "session.started", "control-plane", "attested"),
        ("action", "process.exec", "guest", "observed"),
        ("network", "network.flow", "runtime", "observed"),
        ("runtime", "runtime.resource.sample", "runtime", "observed"),
    )
    for index in range(count):
        plane, event_name, layer, trust = names[index % len(names)]
        collector = f"bench-{layer}"
        sequence = index // len(names) + 1
        events.append(
            {
                "schema_version": SCHEMA_VERSION,
                "event_id": f"0198f000-0000-7000-8000-{index:012x}"[-36:],
                "event_name": event_name,
                "plane": plane,
                "occurred_at": (base + timedelta(microseconds=index)).isoformat().replace("+00:00", "Z"),
                "observed_at": (base + timedelta(microseconds=index + 10)).isoformat().replace("+00:00", "Z"),
                "source": {
                    "collector": collector,
                    "layer": layer,
                    "runtime": "docker",
                    "trust": trust,
                },
                "correlation": {
                    "tenant_id": "tenant-bench",
                    "host_id": "host-bench",
                    "instance_id": "instance-bench",
                    "agent_id": "agent-bench",
                    "session_id": "session-bench",
                },
                "sensitivity": "metadata",
                "retention_class": "standard",
                "payload": {"index": index, "authorization": "must-not-persist"},
                "integrity": {"collector_sequence": sequence},
            }
        )
    return events


def benchmark(counts: Iterable[int], repetitions: int) -> dict[str, Any]:
    cases = []
    for count in counts:
        events = _synthetic_events(count)
        input_bytes = sum(len(canonical_bytes(event)) + 1 for event in events)
        wall_samples: list[float] = []
        cpu_samples: list[float] = []
        peak_samples: list[int] = []
        output_bytes = 0
        for _ in range(repetitions):
            tracemalloc.start()
            wall_start = time.perf_counter()
            cpu_start = time.process_time()
            timeline = build_timeline(events)
            cpu_samples.append(time.process_time() - cpu_start)
            wall_samples.append(time.perf_counter() - wall_start)
            _, peak = tracemalloc.get_traced_memory()
            tracemalloc.stop()
            peak_samples.append(peak)
            output_bytes = len(canonical_bytes(timeline))
        p50_wall = statistics.median(wall_samples)
        p95_wall = _percentile(wall_samples, 0.95)
        cases.append(
            {
                "events": count,
                "repetitions": repetitions,
                "wall_seconds_p50": p50_wall,
                "wall_seconds_p95": p95_wall,
                "cpu_seconds_p50": statistics.median(cpu_samples),
                "events_per_second_p50": count / p50_wall if p50_wall else 0,
                "peak_heap_bytes_max": max(peak_samples),
                "input_bytes": input_bytes,
                "input_bytes_per_event": input_bytes / count,
                "output_bytes": output_bytes,
                "output_bytes_per_event": output_bytes / count,
                "serialized_io_expansion_ratio": output_bytes / input_bytes,
            }
        )
    return {
        "benchmark": "activity-timeline-normalizer",
        "schema_version": "activity.benchmark/v1",
        "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "python": sys.version.split()[0],
        "platform": sys.platform,
        "cpu_count": os.cpu_count(),
        "scope": "offline JSONL normalization, redaction, sorting, gap detection, and hash linking; excludes collection and remote storage",
        "cases": cases,
    }


def _write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _timeline_command(args: argparse.Namespace) -> int:
    timeline = build_timeline(load_events([Path(path) for path in args.inputs]))
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    if args.format == "json":
        _write_json(output, timeline)
    else:
        output.write_text(render_markdown(timeline), encoding="utf-8")
    print(
        f"wrote {len(timeline['events'])} events to {output}; "
        f"gaps={timeline['summary']['sequence_gap_count']} "
        f"dropped={timeline['summary']['explicit_dropped_events']}"
    )
    return 0


def _benchmark_command(args: argparse.Namespace) -> int:
    counts = [int(value) for value in args.event_counts.split(",")]
    if any(value < 1 for value in counts) or args.repetitions < 1:
        raise ValidationError("event counts and repetitions must be positive")
    report = benchmark(counts, args.repetitions)
    _write_json(Path(args.output), report)
    print(f"wrote benchmark report to {args.output}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    timeline = subparsers.add_parser("timeline", help="build a correlated timeline")
    timeline.add_argument("inputs", nargs="+", help="input JSONL files")
    timeline.add_argument("--output", required=True, help="output path")
    timeline.add_argument("--format", choices=("markdown", "json"), default="markdown")
    timeline.set_defaults(handler=_timeline_command)

    bench = subparsers.add_parser("benchmark", help="benchmark the normalizer")
    bench.add_argument("--event-counts", default="100,1000,10000")
    bench.add_argument("--repetitions", type=int, default=3)
    bench.add_argument("--output", required=True, help="JSON output path")
    bench.set_defaults(handler=_benchmark_command)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    try:
        return args.handler(args)
    except (OSError, ValidationError) as error:
        parser.error(str(error))
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
