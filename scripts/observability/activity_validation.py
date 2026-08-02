#!/usr/bin/env python3
"""Bounded metadata canaries and disposable activity failure drills.

This runner has no arbitrary-command executor.  Named fixture adapters are the
only executable drill surface; live or host-wide disruption remains manual.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import stat
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any, Callable

import operational_evidence as oe

CONFIG_SCHEMA = "agentic.activity-validation.config/v1"
STATE_SCHEMA = "agentic.activity-validation.state/v1"
RESULT_SCHEMA = "agentic.activity-validation.result/v1"
ALLOWED_EXECUTORS = {
    "fixture_collector_restart",
    "fixture_exporter_outage",
    "fixture_backpressure",
    "fixture_authorization_rejection",
    "fixture_evidence_corruption",
}
ALLOWED_TARGET_CLASSES = {"disposable_fixture", "designated_validation"}
ALLOWED_BLAST_RADII = {"single_fixture", "single_designated_instance"}
FORBIDDEN_PROFILE_KEYS = {"argv", "command", "environment", "shell", "script"}


class ValidationError(RuntimeError):
    """A validation contract, safety boundary, or execution failure."""


def parse_time(value: str) -> dt.datetime:
    return oe.parse_time(value)


def default_state() -> dict[str, Any]:
    return {
        "schema_version": STATE_SCHEMA,
        "next_canary_sequence": 1,
        "last_canary_at": None,
        "outstanding_canaries": [],
        "schedule_last_run": {},
    }


def load_state(output_dir: Path) -> dict[str, Any]:
    path = output_dir / "validation-state.json"
    if not path.exists():
        return default_state()
    state = oe.load_json(path)
    if state.get("schema_version") != STATE_SCHEMA:
        raise ValidationError("unsupported validation state schema")
    return state


def save_state(output_dir: Path, state: dict[str, Any]) -> None:
    oe.atomic_write(output_dir / "validation-state.json", state)


def _contains_forbidden_key(value: Any) -> str | None:
    if isinstance(value, dict):
        for key, child in value.items():
            if str(key).lower() in FORBIDDEN_PROFILE_KEYS:
                return str(key)
            found = _contains_forbidden_key(child)
            if found:
                return found
    elif isinstance(value, list):
        for child in value:
            found = _contains_forbidden_key(child)
            if found:
                return found
    return None


def _exact_name(value: Any, field: str) -> str:
    if not isinstance(value, str) or not value or any(char in value for char in "*?[]"):
        raise ValidationError(f"{field} must be a non-wildcard exact name")
    return value


def validate_config(config: dict[str, Any]) -> None:
    if config.get("schema_version") != CONFIG_SCHEMA:
        raise ValidationError(f"schema_version must be {CONFIG_SCHEMA}")
    if not set(config).issubset(
        {
            "schema_version",
            "canary",
            "targets",
            "profiles",
            "schedules",
            "maximum_runs_per_invocation",
            "operator_bearer_token_file",
        }
    ):
        raise ValidationError("configuration contains unknown top-level fields")
    token_file = config.get("operator_bearer_token_file")
    if token_file is not None and (
        not isinstance(token_file, str) or not Path(token_file).is_absolute()
    ):
        raise ValidationError("operator_bearer_token_file must be an absolute path")
    canary = config.get("canary")
    if not isinstance(canary, dict):
        raise ValidationError("canary must be an object")
    if set(canary) != {"minimum_interval_seconds", "deadline_seconds", "maximum_outstanding"}:
        raise ValidationError("canary contains unknown or missing fields")
    for field in ("minimum_interval_seconds", "deadline_seconds", "maximum_outstanding"):
        if not isinstance(canary.get(field), int) or canary[field] <= 0:
            raise ValidationError(f"canary.{field} must be a positive integer")
    if canary["minimum_interval_seconds"] < 60:
        raise ValidationError("canary minimum interval must be at least 60 seconds")
    if canary["maximum_outstanding"] > 10:
        raise ValidationError("canary maximum outstanding must not exceed 10")

    targets = config.get("targets")
    if not isinstance(targets, dict) or not targets:
        raise ValidationError("targets must be a non-empty object")
    for name, target in targets.items():
        _exact_name(name, "target name")
        if not isinstance(target, dict) or target.get("target_class") not in ALLOWED_TARGET_CLASSES:
            raise ValidationError(f"target {name}: unsupported target class")
        if set(target) != {
            "target_class",
            "disposable",
            "validation_designated",
            "description",
        }:
            raise ValidationError(f"target {name}: unknown or missing fields")
        if target.get("disposable") is not True and target.get("validation_designated") is not True:
            raise ValidationError(f"target {name}: target is not disposable or validation-designated")

    profiles = config.get("profiles")
    if not isinstance(profiles, dict) or set(profiles) != ALLOWED_EXECUTORS:
        raise ValidationError("profiles must define exactly the five built-in executor names")
    for name, profile in profiles.items():
        _exact_name(name, "profile name")
        if not isinstance(profile, dict) or profile.get("executor") != name:
            raise ValidationError(f"profile {name}: executor must equal the fixed profile name")
        if set(profile) != {
            "executor",
            "target_class",
            "hypothesis",
            "steady_state",
            "maximum_duration_seconds",
            "blast_radius",
            "abort_thresholds",
            "rollback",
            "cleanup_verification",
            "expected_evidence",
            "manual_only",
        }:
            raise ValidationError(f"profile {name}: unknown or missing fields")
        forbidden = _contains_forbidden_key(profile)
        if forbidden:
            raise ValidationError(f"profile {name}: forbidden arbitrary-execution key {forbidden}")
        if profile.get("target_class") not in ALLOWED_TARGET_CLASSES:
            raise ValidationError(f"profile {name}: unsupported target class")
        if profile.get("blast_radius") not in ALLOWED_BLAST_RADII:
            raise ValidationError(f"profile {name}: unsupported blast radius")
        duration = profile.get("maximum_duration_seconds")
        if not isinstance(duration, int) or duration <= 0 or duration > 300:
            raise ValidationError(f"profile {name}: maximum duration must be 1..300 seconds")
        for field in ("hypothesis", "steady_state", "rollback"):
            if not isinstance(profile.get(field), str) or not profile[field].strip():
                raise ValidationError(f"profile {name}: {field} is required")
        thresholds = profile.get("abort_thresholds")
        if not isinstance(thresholds, dict) or not thresholds:
            raise ValidationError(f"profile {name}: abort thresholds are required")
        if not all(
            key.startswith("max_") and isinstance(value, (int, float)) and value >= 0
            for key, value in thresholds.items()
        ):
            raise ValidationError(f"profile {name}: invalid abort threshold")
        for field in ("cleanup_verification", "expected_evidence"):
            value = profile.get(field)
            if not isinstance(value, list) or not value or not all(isinstance(item, str) and item for item in value):
                raise ValidationError(f"profile {name}: {field} must be a non-empty string array")
        if not isinstance(profile.get("manual_only"), bool):
            raise ValidationError(f"profile {name}: manual_only must be boolean")

    schedules = config.get("schedules", [])
    if not isinstance(schedules, list):
        raise ValidationError("schedules must be an array")
    schedule_ids: set[str] = set()
    for schedule in schedules:
        if not isinstance(schedule, dict):
            raise ValidationError("each schedule must be an object")
        if set(schedule) != {"id", "profile", "target", "interval_seconds"}:
            raise ValidationError("schedule contains unknown or missing fields")
        identifier = _exact_name(schedule.get("id"), "schedule id")
        if identifier in schedule_ids:
            raise ValidationError("schedule ids must be unique")
        schedule_ids.add(identifier)
        profile_name = _exact_name(schedule.get("profile"), "schedule profile")
        target_name = _exact_name(schedule.get("target"), "schedule target")
        if profile_name not in profiles or target_name not in targets:
            raise ValidationError(f"schedule {identifier}: unknown profile or target")
        if profiles[profile_name]["manual_only"]:
            raise ValidationError(f"schedule {identifier}: manual-only profile cannot be scheduled")
        interval = schedule.get("interval_seconds")
        if not isinstance(interval, int) or interval < 3_600:
            raise ValidationError(f"schedule {identifier}: interval must be at least one hour")
    maximum = config.get("maximum_runs_per_invocation", 1)
    if not isinstance(maximum, int) or not 1 <= maximum <= 5:
        raise ValidationError("maximum_runs_per_invocation must be 1..5")


def uuid7(now: dt.datetime) -> str:
    timestamp_ms = int(now.timestamp() * 1000) & ((1 << 48) - 1)
    random_value = int.from_bytes(os.urandom(10), "big")
    value = timestamp_ms << 80
    value |= 0x7 << 76
    value |= ((random_value >> 62) & 0xFFF) << 64
    value |= 0b10 << 62
    value |= random_value & ((1 << 62) - 1)
    return str(uuid.UUID(int=value))


class HttpActivityClient:
    """Fixed-endpoint activity client; secrets are read from a protected file."""

    def __init__(self, config: dict[str, Any], evidence_config: dict[str, Any]):
        self.base = str(evidence_config["management_url"]).rstrip("/")
        self.timeout = int(evidence_config["request_timeout_seconds"])
        self.scope_headers = oe.scope_headers(evidence_config)
        self.auth_headers: dict[str, str] = {}
        token_file = config.get("operator_bearer_token_file")
        if token_file:
            path = Path(token_file)
            mode = stat.S_IMODE(path.stat().st_mode)
            if mode & 0o077:
                raise ValidationError("operator bearer token file must not be group/world accessible")
            token = path.read_text(encoding="utf-8").strip()
            if not token:
                raise ValidationError("operator bearer token file is empty")
            self.auth_headers["Authorization"] = f"Bearer {token}"

    def request(self, method: str, path: str, body: dict[str, Any] | None = None) -> dict[str, Any]:
        if method not in {"GET", "POST"}:
            raise ValidationError("activity validation client permits only GET and POST")
        data = oe.canonical_json(body) if body is not None else None
        headers = {
            "Accept": "application/json",
            **self.scope_headers,
            **self.auth_headers,
        }
        if data is not None:
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(
            f"{self.base}{path}", data=data, headers=headers, method=method
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                raw = response.read(1024 * 1024)
                if not 200 <= response.status < 300:
                    raise ValidationError(f"activity endpoint returned HTTP {response.status}")
        except urllib.error.HTTPError as error:
            error.read(1024 * 1024)
            raise ValidationError(f"activity endpoint returned HTTP {error.code}") from error
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise ValidationError("activity endpoint did not return an object")
        return value

    def ingest(self, event: dict[str, Any]) -> dict[str, Any]:
        return self.request("POST", "/api/v2/activity/ingest", {"events": [event]})

    def query(self) -> dict[str, Any]:
        query = urllib.parse.urlencode({"event_name": "validation.canary", "limit": 5000})
        return self.request("GET", f"/api/v2/activity/events?{query}")

    def export(self) -> dict[str, Any]:
        return self.request(
            "POST", "/api/v2/activity/export", {"event_name": "validation.canary", "limit": 5000}
        )


def make_sample(
    evidence_config: dict[str, Any],
    evidence_class: str,
    phase: str,
    status: str,
    metrics: dict[str, float],
    details: dict[str, Any],
    now: dt.datetime,
) -> dict[str, Any]:
    if evidence_class not in {"canary", "drill"}:
        raise ValidationError("synthetic runner may create only canary or drill samples")
    sample: dict[str, Any] = {
        "schema_version": oe.SAMPLE_SCHEMA,
        "sample_id": str(uuid.uuid4()),
        "recorded_at": oe.format_time(now),
        "monotonic_ns": time.monotonic_ns(),
        "evidence_class": evidence_class,
        "evidence_origin": "operational",
        "identity": oe.config_identity(evidence_config),
        "sources": [
            {
                "name": f"activity-{evidence_class}-{phase}",
                "kind": evidence_class,
                "required": False,
                "availability": "available" if status in {"success", "start", "rolled_back", "clean"} else "error",
                "latency_ms": metrics.get("latency_ms", 0.0),
                "metrics": metrics,
            }
        ],
        "metrics": metrics,
        "validation": {"phase": phase, "status": status, **details},
    }
    sample["sample_digest"] = oe.digest_value(sample)
    return sample


def record_sample(
    output_dir: Path,
    evidence_config: dict[str, Any],
    evidence_class: str,
    phase: str,
    status: str,
    metrics: dict[str, float],
    details: dict[str, Any],
    now: dt.datetime,
) -> dict[str, Any]:
    sample = make_sample(evidence_config, evidence_class, phase, status, metrics, details, now)
    oe.append_sample(output_dir, sample)
    return sample


def canary_event(evidence_config: dict[str, Any], sequence: int, now: dt.datetime, deadline: dt.datetime) -> dict[str, Any]:
    scope = evidence_config["activity_scope"]
    canary_id = uuid7(now)
    return {
        "schema_version": "activity.event/v1",
        "event_id": canary_id,
        "event_name": "validation.canary",
        "plane": "integrity",
        "occurred_at": oe.format_time(now),
        "observed_at": oe.format_time(now),
        "source": {
            "collector": scope["collector_id"],
            "layer": "host",
            "runtime": "host",
            "trust": "derived",
        },
        "correlation": {
            "tenant_id": scope["tenant_id"],
            "host_id": scope["host_id"],
            "instance_id": scope["instance_id"],
            "agent_id": scope["agent_id"],
        },
        "sensitivity": "metadata",
        "retention_class": "ephemeral",
        "payload": {
            "canary_id": canary_id,
            "expected_deadline": oe.format_time(deadline),
            "synthetic_class": "known_signal",
        },
        "integrity": {"collector_sequence": sequence},
    }


def run_canary(
    config: dict[str, Any],
    evidence_config: dict[str, Any],
    output_dir: Path,
    client: Any,
    *,
    now: dt.datetime | None = None,
    dry_run: bool = False,
) -> dict[str, Any]:
    validate_config(config)
    current = now or oe.utc_now()
    state = load_state(output_dir)
    canary = config["canary"]
    outstanding = [
        item
        for item in state.get("outstanding_canaries", [])
        if parse_time(item["deadline"]) > current
    ]
    last = state.get("last_canary_at")
    if last and (current - parse_time(last)).total_seconds() < canary["minimum_interval_seconds"]:
        raise ValidationError("canary rate budget has not elapsed")
    if len(outstanding) >= canary["maximum_outstanding"]:
        raise ValidationError("maximum outstanding canary count reached")
    sequence = int(state.get("next_canary_sequence", 1))
    deadline = current + dt.timedelta(seconds=canary["deadline_seconds"])
    event = canary_event(evidence_config, sequence, current, deadline)
    plan = {
        "schema_version": RESULT_SCHEMA,
        "kind": "canary",
        "dry_run": dry_run,
        "event_id": event["event_id"],
        "sequence": sequence,
        "deadline": oe.format_time(deadline),
        "maximum_outstanding": canary["maximum_outstanding"],
        "minimum_interval_seconds": canary["minimum_interval_seconds"],
        "endpoints": ["activity-ingest", "activity-query", "activity-export"],
    }
    if dry_run:
        return plan
    pending = {"event_id": event["event_id"], "deadline": oe.format_time(deadline)}
    outstanding.append(pending)
    # Reserve the sequence and rate slot durably before the request. A process
    # crash after a successful ingest must not reuse the sequence with a new ID.
    state["last_canary_at"] = oe.format_time(current)
    state["next_canary_sequence"] = sequence + 1
    state["outstanding_canaries"] = outstanding
    save_state(output_dir, state)
    latencies: dict[str, float] = {}
    error: str | None = None
    visible = {"ingest": False, "query": False, "export": False}
    try:
        started = time.monotonic()
        ack = client.ingest(event)
        latencies["ingest_latency_ms"] = (time.monotonic() - started) * 1000
        visible["ingest"] = int(ack.get("accepted", 0)) + int(ack.get("duplicates", 0)) == 1
        started = time.monotonic()
        query = client.query()
        latencies["query_latency_ms"] = (time.monotonic() - started) * 1000
        visible["query"] = any(item.get("event_id") == event["event_id"] for item in query.get("events", []))
        started = time.monotonic()
        exported = client.export()
        latencies["export_latency_ms"] = (time.monotonic() - started) * 1000
        visible["export"] = any(item.get("event_id") == event["event_id"] for item in exported.get("events", []))
        if not all(visible.values()):
            raise ValidationError("canary was not visible across ingest/query/export")
    except (OSError, ValueError, ValidationError) as caught:
        error = str(caught)
    status = "success" if error is None else "failure"
    if status == "success":
        outstanding = [item for item in outstanding if item["event_id"] != event["event_id"]]
    state["outstanding_canaries"] = outstanding
    save_state(output_dir, state)
    metrics = {
        **latencies,
        "latency_ms": sum(latencies.values()),
        "canary_success": 1.0 if status == "success" else 0.0,
        "canary_outstanding": float(len(outstanding)),
    }
    record_sample(
        output_dir,
        evidence_config,
        "canary",
        "verification",
        status,
        metrics,
        {"event_id": event["event_id"], "visibility": visible, "error": error},
        current,
    )
    return {**plan, "status": status, "visibility": visible, "metrics": metrics, "error": error}


class FixtureDrillAdapter:
    """In-process, reversible fixture only; it never touches host services."""

    def __init__(self, executor: str):
        if executor not in ALLOWED_EXECUTORS:
            raise ValidationError("unknown drill executor")
        self.executor = executor
        self.state: dict[str, Any] = {
            "collector_running": True,
            "exporter_available": True,
            "queue_depth": 0,
            "authorized_identity": True,
            "protected_state": "unchanged",
            "mutated_copy": None,
        }

    def start(self, profile: dict[str, Any], target: dict[str, Any]) -> dict[str, Any]:
        metrics = {"error_count": 0.0, "latency_ms": 1.0, "queue_depth": 0.0}
        if self.executor == "fixture_collector_restart":
            self.state["collector_running"] = False
            evidence = ["collector_restart"]
        elif self.executor == "fixture_exporter_outage":
            self.state["exporter_available"] = False
            evidence = ["exporter_unavailable"]
        elif self.executor == "fixture_backpressure":
            self.state["queue_depth"] = 50
            metrics["queue_depth"] = 50.0
            evidence = ["backpressure_applied"]
        elif self.executor == "fixture_authorization_rejection":
            self.state["authorized_identity"] = False
            evidence = ["authorization_rejected", "denial_audited"]
        else:
            original = {"record": "fixture", "digest": "trusted"}
            self.state["mutated_copy"] = {**original, "digest": "mutated"}
            evidence = ["mutation_detected", "original_verified"]
        return {"metrics": metrics, "evidence": evidence}

    def rollback(self, profile: dict[str, Any], target: dict[str, Any]) -> bool:
        self.state.update(
            {
                "collector_running": True,
                "exporter_available": True,
                "queue_depth": 0,
                "authorized_identity": True,
                "protected_state": "unchanged",
                "mutated_copy": None,
            }
        )
        return True

    def cleanup(self, profile: dict[str, Any], target: dict[str, Any]) -> bool:
        return self.state == {
            "collector_running": True,
            "exporter_available": True,
            "queue_depth": 0,
            "authorized_identity": True,
            "protected_state": "unchanged",
            "mutated_copy": None,
        }


def abort_reasons(profile: dict[str, Any], observation: dict[str, Any], elapsed: float) -> list[str]:
    reasons: list[str] = []
    if elapsed > profile["maximum_duration_seconds"]:
        reasons.append("maximum duration exceeded")
    metrics = observation.get("metrics", {})
    for threshold, maximum in profile["abort_thresholds"].items():
        metric = threshold.removeprefix("max_")
        value = metrics.get(metric)
        if isinstance(value, (int, float)) and value > maximum:
            reasons.append(f"{metric} exceeded {maximum}")
    missing = sorted(set(profile["expected_evidence"]) - set(observation.get("evidence", [])))
    if missing:
        reasons.append(f"missing expected evidence: {', '.join(missing)}")
    return reasons


def drill_plan(config: dict[str, Any], profile_name: str, target_name: str) -> dict[str, Any]:
    validate_config(config)
    profile_name = _exact_name(profile_name, "profile name")
    target_name = _exact_name(target_name, "target name")
    profile = config["profiles"].get(profile_name)
    target = config["targets"].get(target_name)
    if profile is None or target is None:
        raise ValidationError("unknown profile or target")
    if profile["target_class"] != target["target_class"]:
        raise ValidationError("profile target class does not match exact target designation")
    return {
        "schema_version": RESULT_SCHEMA,
        "kind": "drill",
        "profile": profile_name,
        "target": target_name,
        "executor": profile["executor"],
        "target_class": profile["target_class"],
        "hypothesis": profile["hypothesis"],
        "steady_state": profile["steady_state"],
        "maximum_duration_seconds": profile["maximum_duration_seconds"],
        "blast_radius": profile["blast_radius"],
        "abort_thresholds": profile["abort_thresholds"],
        "rollback": profile["rollback"],
        "cleanup_verification": profile["cleanup_verification"],
        "expected_evidence": profile["expected_evidence"],
        "manual_only": profile["manual_only"],
    }


def run_drill(
    config: dict[str, Any],
    evidence_config: dict[str, Any],
    output_dir: Path,
    profile_name: str,
    target_name: str,
    *,
    adapter: Any | None = None,
    dry_run: bool = False,
    now: dt.datetime | None = None,
    monotonic: Callable[[], float] = time.monotonic,
) -> dict[str, Any]:
    plan = drill_plan(config, profile_name, target_name)
    plan["dry_run"] = dry_run
    if dry_run:
        return plan
    current = now or oe.utc_now()
    profile = config["profiles"][profile_name]
    target = config["targets"][target_name]
    runner = adapter or FixtureDrillAdapter(profile["executor"])
    details = {"profile": profile_name, "target": target_name}
    record_sample(output_dir, evidence_config, "drill", "start", "start", {}, details, current)
    started = monotonic()
    observation: dict[str, Any] = {"metrics": {}, "evidence": []}
    failure: str | None = None
    try:
        observation = runner.start(profile, target)
    except Exception as error:  # adapter boundary is recorded, then always rolled back
        failure = f"executor failure: {type(error).__name__}"
    elapsed = max(0.0, monotonic() - started)
    reasons = abort_reasons(profile, observation, elapsed)
    if failure:
        reasons.append(failure)
    if reasons:
        record_sample(
            output_dir,
            evidence_config,
            "drill",
            "abort",
            "aborted",
            {**observation.get("metrics", {}), "latency_ms": elapsed * 1000},
            {**details, "reasons": reasons},
            current,
        )
    rollback_ok = False
    cleanup_ok = False
    try:
        rollback_ok = runner.rollback(profile, target) is True
    except Exception:
        rollback_ok = False
    record_sample(
        output_dir,
        evidence_config,
        "drill",
        "rollback",
        "rolled_back" if rollback_ok else "rollback_failed",
        {},
        details,
        current,
    )
    try:
        cleanup_ok = runner.cleanup(profile, target) is True
    except Exception:
        cleanup_ok = False
    record_sample(
        output_dir,
        evidence_config,
        "drill",
        "cleanup",
        "clean" if cleanup_ok else "cleanup_failed",
        {},
        details,
        current,
    )
    if not rollback_ok:
        status = "rollback_failed"
    elif not cleanup_ok:
        status = "cleanup_failed"
    elif reasons:
        status = "aborted"
    else:
        status = "success"
    record_sample(
        output_dir,
        evidence_config,
        "drill",
        "complete",
        status,
        {**observation.get("metrics", {}), "latency_ms": elapsed * 1000},
        {**details, "abort_reasons": reasons, "rollback_ok": rollback_ok, "cleanup_ok": cleanup_ok},
        current,
    )
    return {
        **plan,
        "status": status,
        "abort_reasons": reasons,
        "rollback_ok": rollback_ok,
        "cleanup_ok": cleanup_ok,
        "elapsed_seconds": elapsed,
    }


def run_schedule_once(
    config: dict[str, Any],
    evidence_config: dict[str, Any],
    output_dir: Path,
    *,
    now: dt.datetime | None = None,
    dry_run: bool = False,
) -> dict[str, Any]:
    validate_config(config)
    current = now or oe.utc_now()
    state = load_state(output_dir)
    results: list[dict[str, Any]] = []
    for schedule in config.get("schedules", []):
        last_raw = state.get("schedule_last_run", {}).get(schedule["id"])
        due = last_raw is None or (current - parse_time(last_raw)).total_seconds() >= schedule["interval_seconds"]
        if not due:
            continue
        results.append(
            run_drill(
                config,
                evidence_config,
                output_dir,
                schedule["profile"],
                schedule["target"],
                dry_run=dry_run,
                now=current,
            )
        )
        if not dry_run:
            state.setdefault("schedule_last_run", {})[schedule["id"]] = oe.format_time(current)
            save_state(output_dir, state)
        if len(results) >= config.get("maximum_runs_per_invocation", 1):
            break
    return {"schema_version": RESULT_SCHEMA, "kind": "schedule", "dry_run": dry_run, "runs": results}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--evidence-config", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    commands = parser.add_subparsers(dest="command", required=True)
    canary = commands.add_parser("canary")
    canary.add_argument("--dry-run", action="store_true")
    drill = commands.add_parser("drill")
    drill.add_argument("--profile", required=True)
    drill.add_argument("--target", required=True)
    drill.add_argument("--dry-run", action="store_true")
    schedule = commands.add_parser("schedule-once")
    schedule.add_argument("--dry-run", action="store_true")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    config = oe.load_json(args.config)
    evidence_config = oe.load_json(args.evidence_config)
    validate_config(config)
    oe.validate_config(evidence_config)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    if args.command == "canary":
        client = HttpActivityClient(config, evidence_config)
        result = run_canary(config, evidence_config, args.output_dir, client, dry_run=args.dry_run)
    elif args.command == "drill":
        result = run_drill(
            config,
            evidence_config,
            args.output_dir,
            args.profile,
            args.target,
            dry_run=args.dry_run,
        )
    else:
        result = run_schedule_once(
            config, evidence_config, args.output_dir, dry_run=args.dry_run
        )
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (oe.EvidenceError, ValidationError) as error:
        raise SystemExit(f"activity-validation: {error}") from error
