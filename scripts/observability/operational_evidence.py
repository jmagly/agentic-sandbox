#!/usr/bin/env python3
"""Passive rolling evidence collection for persistent Agentic Sandbox agents.

Only GET requests and local read-only inspection are implemented here.  The
active capacity harness remains a separate program by design.
"""

from __future__ import annotations

import argparse
import datetime as dt
import fcntl
import hashlib
import json
import math
import os
import statistics
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any, Iterable

CONFIG_SCHEMA = "agentic.activity-operational-evidence.config/v1"
SAMPLE_SCHEMA = "agentic.activity-operational-evidence.sample/v1"
RECORD_SCHEMA = "agentic.activity-operational-evidence/v1"
REPORT_SCHEMA = "agentic.activity-operational-evidence.report/v1"
SAMPLER_VERSION = "1.0.0"
WINDOWS = {"1h": 3_600, "24h": 86_400, "7d": 604_800}
EVIDENCE_CLASSES = {"organic", "canary", "drill"}


class EvidenceError(RuntimeError):
    """An evidence contract or integrity failure."""


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode("utf-8")


def digest_value(value: Any) -> str:
    return hashlib.sha256(canonical_json(value)).hexdigest()


def parse_time(value: str) -> dt.datetime:
    parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise EvidenceError("timestamps must include a UTC offset")
    return parsed.astimezone(dt.timezone.utc)


def format_time(value: dt.datetime) -> str:
    return value.astimezone(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def utc_now() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc)


def load_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise EvidenceError(f"{path}: expected a JSON object")
    return value


def validate_http_url(value: str, field: str) -> None:
    parsed = urllib.parse.urlparse(value)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise EvidenceError(f"{field} must be an HTTP(S) URL")
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise EvidenceError(
            f"{field} must not include credentials, query data, or fragments"
        )


def validate_config(config: dict[str, Any]) -> None:
    if config.get("schema_version") != CONFIG_SCHEMA:
        raise EvidenceError(f"config schema_version must be {CONFIG_SCHEMA}")
    for field in ("sample_interval_seconds", "request_timeout_seconds"):
        if not isinstance(config.get(field), int) or config[field] <= 0:
            raise EvidenceError(f"{field} must be a positive integer")
    validate_http_url(str(config.get("management_url", "")), "management_url")
    prometheus_url = config.get("prometheus_url")
    if prometheus_url is not None:
        validate_http_url(str(prometheus_url), "prometheus_url")
    scope = config.get("activity_scope")
    if not isinstance(scope, dict):
        raise EvidenceError("activity_scope must be an object")
    for field in ("tenant_id", "host_id", "instance_id", "agent_id", "collector_id"):
        value = scope.get(field)
        if not isinstance(value, str) or not value.strip() or len(value) > 255:
            raise EvidenceError(f"activity_scope.{field} must be a non-empty string")
    sources = config.get("sources")
    if not isinstance(sources, list) or not sources:
        raise EvidenceError("sources must be a non-empty array")
    names: set[str] = set()
    for source in sources:
        if not isinstance(source, dict):
            raise EvidenceError("each source must be an object")
        name = source.get("name")
        if not isinstance(name, str) or not name or name in names:
            raise EvidenceError("source names must be non-empty and unique")
        names.add(name)
        if source.get("kind") not in {"health", "activity_coverage", "prometheus"}:
            raise EvidenceError(f"source {name}: unsupported read-only kind")
        if not isinstance(source.get("required", True), bool):
            raise EvidenceError(f"source {name}: required must be boolean")
        path = source.get("path")
        if source.get("kind") != "prometheus":
            if not isinstance(path, str) or not path.startswith("/"):
                raise EvidenceError(f"source {name}: path must be absolute")
        elif not isinstance(source.get("query"), str) or not source["query"].strip():
            raise EvidenceError(f"source {name}: query is required")
    for path in config.get("storage_paths", []):
        if not isinstance(path, str) or not path.startswith("/"):
            raise EvidenceError("storage_paths must contain absolute paths")
    identity = config.get("identity")
    if not isinstance(identity, dict):
        raise EvidenceError("identity must be an object")
    for field in ("runtime", "environment", "collector_tier"):
        if not isinstance(identity.get(field), str) or not identity[field]:
            raise EvidenceError(f"identity.{field} must be a non-empty string")
    cost = config.get("cost", {})
    for field in ("storage_usd_per_gib_month", "compute_usd_per_hour"):
        value = cost.get(field, 0.0)
        if not isinstance(value, (int, float)) or value < 0 or not math.isfinite(value):
            raise EvidenceError(f"cost.{field} must be a finite non-negative number")


def config_identity(config: dict[str, Any]) -> dict[str, Any]:
    return {
        "config_sha256": digest_value(config),
        "implementation_commit": git_commit(),
        "sampler_version": SAMPLER_VERSION,
        "runtime": config["identity"]["runtime"],
        "environment": config["identity"]["environment"],
        "collector_tier": config["identity"]["collector_tier"],
        "scope": dict(config["activity_scope"]),
    }


def git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def scope_headers(config: dict[str, Any]) -> dict[str, str]:
    scope = config["activity_scope"]
    return {
        "x-activity-tenant-id": scope["tenant_id"],
        "x-activity-host-id": scope["host_id"],
        "x-activity-instance-id": scope["instance_id"],
        "x-activity-agent-id": scope["agent_id"],
        "x-activity-collector-id": scope["collector_id"],
    }


def read_json_get(
    url: str, timeout: int, headers: dict[str, str] | None = None
) -> tuple[int, dict[str, Any] | None, float, str]:
    request_headers = {"Accept": "application/json", **(headers or {})}
    request = urllib.request.Request(url, headers=request_headers, method="GET")
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status = response.status
            raw = response.read(1024 * 1024)
    except urllib.error.HTTPError as error:
        status = error.code
        raw = error.read(1024 * 1024)
    latency_ms = (time.monotonic() - started) * 1_000
    parsed: dict[str, Any] | None = None
    outcome = "available" if 200 <= status < 300 else "error"
    if raw:
        try:
            candidate = json.loads(raw)
            if isinstance(candidate, dict):
                parsed = candidate
        except (UnicodeDecodeError, json.JSONDecodeError):
            outcome = "corrupt"
    return status, parsed, latency_ms, outcome


def finite_number(value: Any) -> float | None:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if math.isfinite(number) else None


def activity_metrics(payload: dict[str, Any] | None) -> dict[str, float]:
    completeness = (payload or {}).get("completeness", {})
    if not isinstance(completeness, dict):
        completeness = {}
    result: dict[str, float] = {}
    for field in (
        "sequence_gap_count",
        "durable_loss_count",
        "restart_count",
        "dropped_event_count",
        "stale_collector_count",
        "maximum_clock_error_ms",
    ):
        value = finite_number(completeness.get(field))
        if value is not None:
            result[field] = value
    result["coverage_complete"] = 1.0 if completeness.get("complete") is True else 0.0
    return result


def prometheus_value(payload: dict[str, Any] | None) -> float | None:
    values: list[float] = []
    for item in ((payload or {}).get("data", {}).get("result", [])):
        if not isinstance(item, dict):
            continue
        raw = item.get("value", [None, None])
        if isinstance(raw, list) and len(raw) > 1:
            value = finite_number(raw[1])
            if value is not None:
                values.append(value)
    return statistics.fmean(values) if values else None


def collect_source(config: dict[str, Any], source: dict[str, Any]) -> dict[str, Any]:
    started = time.monotonic()
    kind = source["kind"]
    try:
        if kind == "prometheus":
            base = config.get("prometheus_url")
            if not base:
                return {
                    "name": source["name"],
                    "kind": kind,
                    "required": source.get("required", True),
                    "availability": "unsupported",
                    "latency_ms": 0.0,
                    "metrics": {},
                }
            query = urllib.parse.urlencode({"query": source["query"]})
            url = f"{str(base).rstrip('/')}/api/v1/query?{query}"
            status, payload, latency, availability = read_json_get(
                url, config["request_timeout_seconds"]
            )
            value = prometheus_value(payload)
            if value is None and availability == "available":
                availability = "missing"
            metrics = {source.get("metric", source["name"]): value} if value is not None else {}
        else:
            url = f"{config['management_url'].rstrip('/')}{source['path']}"
            headers = scope_headers(config) if kind == "activity_coverage" else {}
            status, payload, latency, availability = read_json_get(
                url, config["request_timeout_seconds"], headers
            )
            metrics = activity_metrics(payload) if kind == "activity_coverage" else {}
        return {
            "name": source["name"],
            "kind": kind,
            "required": source.get("required", True),
            "availability": availability,
            "status": status,
            "latency_ms": round(latency, 3),
            "metrics": metrics,
            "input_digest": digest_value(payload) if payload is not None else None,
        }
    except (OSError, TimeoutError, urllib.error.URLError) as error:
        return {
            "name": source["name"],
            "kind": kind,
            "required": source.get("required", True),
            "availability": "error",
            "error_class": type(error).__name__,
            "latency_ms": round((time.monotonic() - started) * 1_000, 3),
            "metrics": {},
        }


def process_metrics(config: dict[str, Any]) -> dict[str, float]:
    pid = config.get("management_pid")
    pid_file = config.get("management_pid_file")
    if pid is None and isinstance(pid_file, str):
        try:
            pid = int(Path(pid_file).read_text(encoding="utf-8").strip())
        except (OSError, ValueError):
            pid = None
    if not isinstance(pid, int) or pid <= 0:
        return {}
    result: dict[str, float] = {}
    try:
        for line in Path(f"/proc/{pid}/status").read_text(encoding="utf-8").splitlines():
            if line.startswith("VmRSS:"):
                result["management_rss_bytes"] = float(line.split()[1]) * 1024
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8").split()
        ticks = os.sysconf(os.sysconf_names["SC_CLK_TCK"])
        result["management_cpu_seconds"] = (float(stat[13]) + float(stat[14])) / ticks
    except (OSError, IndexError, ValueError):
        return {}
    return result


def storage_metrics(config: dict[str, Any], output_dir: Path) -> dict[str, float]:
    result: dict[str, float] = {}
    total = 0
    for configured in config.get("storage_paths", []):
        base = Path(configured)
        for suffix in ("", "-wal", "-shm"):
            candidate = Path(f"{base}{suffix}")
            try:
                size = candidate.stat().st_size
            except OSError:
                continue
            total += size
    result["activity_storage_bytes"] = float(total)
    artifact_total = 0
    if output_dir.exists():
        for path in output_dir.rglob("*"):
            try:
                if path.is_file():
                    artifact_total += path.stat().st_size
            except OSError:
                continue
    result["evidence_artifact_bytes"] = float(artifact_total)
    cost = config.get("cost", {})
    result["estimated_storage_usd_per_month"] = (
        (total + artifact_total)
        / float(1024**3)
        * float(cost.get("storage_usd_per_gib_month", 0.0))
    )
    result["estimated_compute_usd_per_hour"] = float(
        cost.get("compute_usd_per_hour", 0.0)
    )
    return result


def create_sample(
    config: dict[str, Any],
    output_dir: Path,
    *,
    evidence_class: str = "organic",
    now: dt.datetime | None = None,
    monotonic_ns: int | None = None,
) -> dict[str, Any]:
    if evidence_class not in EVIDENCE_CLASSES:
        raise EvidenceError(f"unsupported evidence class {evidence_class}")
    if evidence_class != "organic":
        raise EvidenceError("passive collector may only create organic samples")
    wall = now or utc_now()
    monotonic_value = monotonic_ns if monotonic_ns is not None else time.monotonic_ns()
    sources = [collect_source(config, source) for source in config["sources"]]
    metrics = {**process_metrics(config), **storage_metrics(config, output_dir)}
    sample: dict[str, Any] = {
        "schema_version": SAMPLE_SCHEMA,
        "sample_id": str(uuid.uuid4()),
        "recorded_at": format_time(wall),
        "monotonic_ns": monotonic_value,
        "evidence_class": evidence_class,
        "evidence_origin": "operational",
        "identity": config_identity(config),
        "sources": sources,
        "metrics": metrics,
    }
    sample["sample_digest"] = digest_value(sample)
    return sample


def validate_sample(sample: dict[str, Any]) -> None:
    if sample.get("schema_version") != SAMPLE_SCHEMA:
        raise EvidenceError("unsupported sample schema")
    if sample.get("evidence_class") not in EVIDENCE_CLASSES:
        raise EvidenceError("invalid evidence class")
    if sample.get("evidence_origin") not in {"operational", "fixture"}:
        raise EvidenceError("invalid evidence origin")
    expected = sample.get("sample_digest")
    unsigned = dict(sample)
    unsigned.pop("sample_digest", None)
    if not isinstance(expected, str) or digest_value(unsigned) != expected:
        raise EvidenceError("sample digest mismatch")
    parse_time(str(sample.get("recorded_at", "")))


def atomic_write(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    data = canonical_json(value) + b"\n"
    with temporary.open("wb") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)
    directory_fd = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def append_sample(output_dir: Path, sample: dict[str, Any]) -> Path:
    validate_sample(sample)
    date = parse_time(sample["recorded_at"]).date().isoformat()
    path = output_dir / "samples" / f"{date}.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = output_dir / ".ledger.lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+", encoding="utf-8") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        with path.open("ab") as handle:
            handle.write(canonical_json(sample) + b"\n")
            handle.flush()
            os.fsync(handle.fileno())
        fcntl.flock(lock, fcntl.LOCK_UN)
    return path


def iter_samples(output_dir: Path) -> list[dict[str, Any]]:
    samples: list[dict[str, Any]] = []
    for path in sorted((output_dir / "samples").glob("*.jsonl")):
        with path.open(encoding="utf-8") as handle:
            for number, line in enumerate(handle, 1):
                try:
                    sample = json.loads(line)
                except json.JSONDecodeError as error:
                    raise EvidenceError(f"{path}:{number}: corrupt JSON: {error}") from error
                if not isinstance(sample, dict):
                    raise EvidenceError(f"{path}:{number}: expected object")
                validate_sample(sample)
                samples.append(sample)
    samples.sort(key=lambda sample: parse_time(sample["recorded_at"]))
    return samples


def percentile(values: Iterable[float], percentile_value: float) -> float | None:
    ordered = sorted(values)
    if not ordered:
        return None
    rank = max(0, math.ceil(percentile_value * len(ordered)) - 1)
    return ordered[rank]


def summarize(samples: list[dict[str, Any]]) -> dict[str, Any]:
    classes = {name: 0 for name in sorted(EVIDENCE_CLASSES)}
    latencies: list[float] = []
    metrics: dict[str, list[float]] = {}
    source_availability: dict[str, dict[str, int]] = {}
    for sample in samples:
        classes[sample["evidence_class"]] += 1
        for source in sample.get("sources", []):
            name = source.get("name", "unknown")
            state = source.get("availability", "error")
            source_availability.setdefault(name, {}).setdefault(state, 0)
            source_availability[name][state] += 1
            value = finite_number(source.get("latency_ms"))
            if value is not None:
                latencies.append(value)
            for metric, raw in source.get("metrics", {}).items():
                value = finite_number(raw)
                if value is not None:
                    metrics.setdefault(metric, []).append(value)
        for metric, raw in sample.get("metrics", {}).items():
            value = finite_number(raw)
            if value is not None:
                metrics.setdefault(metric, []).append(value)
    return {
        "sample_count": len(samples),
        "evidence_class_counts": classes,
        "source_availability": source_availability,
        "latency_ms": {
            "p50": percentile(latencies, 0.50),
            "p95": percentile(latencies, 0.95),
            "p99": percentile(latencies, 0.99),
        },
        "metrics": {
            name: {"minimum": min(values), "maximum": max(values), "latest": values[-1]}
            for name, values in sorted(metrics.items())
        },
    }


def load_manifest(output_dir: Path) -> dict[str, Any]:
    path = output_dir / "manifest.json"
    if not path.exists():
        return {
            "schema_version": RECORD_SCHEMA,
            "record_count": 0,
            "head_digest": None,
            "records": [],
        }
    return load_json(path)


def seal_day(output_dir: Path, date: dt.date) -> dict[str, Any]:
    samples = [
        sample
        for sample in iter_samples(output_dir)
        if parse_time(sample["recorded_at"]).date() == date
    ]
    if not samples:
        raise EvidenceError(f"no samples available for {date.isoformat()}")
    manifest = load_manifest(output_dir)
    record_path = output_dir / "records" / f"{date.isoformat()}.json"
    if record_path.exists():
        raise EvidenceError(f"record already sealed for {date.isoformat()}")
    identity_digests = {digest_value(sample["identity"]) for sample in samples}
    record: dict[str, Any] = {
        "schema_version": RECORD_SCHEMA,
        "record_date": date.isoformat(),
        "sealed_at": format_time(utc_now()),
        "window": {
            "start": samples[0]["recorded_at"],
            "end": samples[-1]["recorded_at"],
            "actual_span_seconds": max(
                0.0,
                (
                    parse_time(samples[-1]["recorded_at"])
                    - parse_time(samples[0]["recorded_at"])
                ).total_seconds(),
            ),
        },
        "identity_digests": sorted(identity_digests),
        "input_digest": digest_value([sample["sample_digest"] for sample in samples]),
        "previous_record_digest": manifest.get("head_digest"),
        "summary": summarize(samples),
    }
    record["record_digest"] = digest_value(record)
    atomic_write(record_path, record)
    manifest["record_count"] = int(manifest.get("record_count", 0)) + 1
    manifest["head_digest"] = record["record_digest"]
    manifest.setdefault("records", []).append(
        {
            "date": date.isoformat(),
            "path": str(record_path.relative_to(output_dir)),
            "digest": record["record_digest"],
        }
    )
    atomic_write(output_dir / "manifest.json", manifest)
    return record


def verify_records(output_dir: Path) -> dict[str, Any]:
    manifest = load_manifest(output_dir)
    previous: str | None = None
    verified = 0
    for entry in manifest.get("records", []):
        path = output_dir / entry["path"]
        record = load_json(path)
        expected = record.get("record_digest")
        unsigned = dict(record)
        unsigned.pop("record_digest", None)
        if expected != digest_value(unsigned) or expected != entry.get("digest"):
            raise EvidenceError(f"record digest mismatch: {path}")
        if record.get("previous_record_digest") != previous:
            raise EvidenceError(f"record chain mismatch: {path}")
        previous = expected
        verified += 1
    if int(manifest.get("record_count", -1)) != verified:
        raise EvidenceError("manifest record_count mismatch")
    if manifest.get("head_digest") != previous:
        raise EvidenceError("manifest head_digest mismatch")
    return {"verified": True, "record_count": verified, "head_digest": previous}


def required_sources_available(sample: dict[str, Any]) -> bool:
    return all(
        source.get("availability") == "available"
        for source in sample.get("sources", [])
        if source.get("required", True)
    )


def threshold_failures(
    samples: list[dict[str, Any]], thresholds: dict[str, Any]
) -> list[str]:
    failures: list[str] = []
    values: dict[str, list[float]] = {}
    for sample in samples:
        for source in sample.get("sources", []):
            for name, raw in source.get("metrics", {}).items():
                value = finite_number(raw)
                if value is not None:
                    values.setdefault(name, []).append(value)
            latency = finite_number(source.get("latency_ms"))
            if latency is not None:
                values.setdefault("source_latency_ms", []).append(latency)
        for name, raw in sample.get("metrics", {}).items():
            value = finite_number(raw)
            if value is not None:
                values.setdefault(name, []).append(value)
    for name, maximum in thresholds.get("maximum", {}).items():
        if name in values and max(values[name]) > float(maximum):
            failures.append(f"{name} exceeded maximum {maximum}")
    for name, minimum in thresholds.get("minimum", {}).items():
        if name in values and min(values[name]) < float(minimum):
            failures.append(f"{name} fell below minimum {minimum}")
    return failures


def evaluate_window(
    samples: list[dict[str, Any]],
    config: dict[str, Any],
    end: dt.datetime,
    duration_seconds: int,
) -> dict[str, Any]:
    start = end - dt.timedelta(seconds=duration_seconds)
    selected = [
        sample
        for sample in samples
        if start <= parse_time(sample["recorded_at"]) <= end
    ]
    organic = [
        sample
        for sample in selected
        if sample.get("evidence_class") == "organic"
        and sample.get("evidence_origin") == "operational"
    ]
    reasons: list[str] = []
    interruptions: list[dict[str, Any]] = []
    interval = config["sample_interval_seconds"]
    max_gap = float(config.get("maximum_sample_gap_seconds", interval * 1.5))
    for left, right in zip(organic, organic[1:]):
        left_time = parse_time(left["recorded_at"])
        right_time = parse_time(right["recorded_at"])
        gap = (right_time - left_time).total_seconds()
        if gap > max_gap:
            interruptions.append(
                {
                    "type": "wall_clock_gap",
                    "start": left["recorded_at"],
                    "end": right["recorded_at"],
                    "seconds": gap,
                }
            )
        left_mono = left.get("monotonic_ns")
        right_mono = right.get("monotonic_ns")
        if isinstance(left_mono, int) and isinstance(right_mono, int):
            monotonic_delta = (right_mono - left_mono) / 1_000_000_000
            if monotonic_delta < 0 or abs(monotonic_delta - gap) > max_gap:
                interruptions.append(
                    {
                        "type": "clock_discontinuity",
                        "start": left["recorded_at"],
                        "end": right["recorded_at"],
                        "wall_seconds": gap,
                        "monotonic_seconds": monotonic_delta,
                    }
                )
    actual_span = (
        (parse_time(organic[-1]["recorded_at"]) - parse_time(organic[0]["recorded_at"])).total_seconds()
        if len(organic) > 1
        else 0.0
    )
    if actual_span < duration_seconds:
        reasons.append(
            f"actual consecutive wall-clock span {actual_span:.3f}s is below {duration_seconds}s"
        )
    if interruptions:
        reasons.append("sample interruptions exceed the configured maximum gap")
    unavailable = [
        sample["recorded_at"]
        for sample in organic
        if not required_sources_available(sample)
    ]
    if unavailable:
        reasons.append(f"required source unavailable in {len(unavailable)} sample(s)")
    if any(sample.get("evidence_origin") != "operational" for sample in selected):
        reasons.append("fixture evidence is present and cannot qualify wall-clock duration")
    identities = {digest_value(sample.get("identity")) for sample in organic}
    if len(identities) > 1:
        reasons.append("build/config/runtime/collector identity changed inside the window")
    failures = threshold_failures(selected, config.get("thresholds", {}))
    status = "insufficient_evidence" if reasons else ("fail" if failures else "pass")
    expected_samples = math.floor(duration_seconds / interval) + 1
    available_samples = sum(required_sources_available(sample) for sample in organic)
    return {
        "status": status,
        "window": {"start": format_time(start), "end": format_time(end)},
        "required_duration_seconds": duration_seconds,
        "actual_consecutive_seconds": actual_span if not interruptions else 0.0,
        "expected_samples": expected_samples,
        "observed_samples": len(organic),
        "available_samples": available_samples,
        "coverage_ratio": min(1.0, available_samples / expected_samples),
        "interruptions": interruptions,
        "reasons": reasons,
        "threshold_failures": failures,
        "summary": summarize(selected),
    }


def evaluate(
    config: dict[str, Any], output_dir: Path, as_of: dt.datetime | None = None
) -> dict[str, Any]:
    samples = iter_samples(output_dir)
    end = as_of or (parse_time(samples[-1]["recorded_at"]) if samples else utc_now())
    chain = verify_records(output_dir)
    report = {
        "schema_version": REPORT_SCHEMA,
        "generated_at": format_time(utc_now()),
        "as_of": format_time(end),
        "identity": config_identity(config),
        "record_chain": chain,
        "windows": {
            name: evaluate_window(samples, config, end, seconds)
            for name, seconds in WINDOWS.items()
        },
        "limits": [
            "Only actual consecutive wall-clock samples can satisfy duration qualification.",
            "Canary and drill samples are never counted as organic traffic.",
            "Unsupported and missing required sources are insufficient evidence.",
        ],
    }
    report["report_digest"] = digest_value(report)
    atomic_write(output_dir / "reports" / "latest.json", report)
    return report


def run_loop(config: dict[str, Any], output_dir: Path, duration: int) -> int:
    started = time.monotonic()
    while True:
        append_sample(output_dir, create_sample(config, output_dir))
        evaluate(config, output_dir)
        if duration and time.monotonic() - started >= duration:
            break
        time.sleep(config["sample_interval_seconds"])
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("collect-once")
    seal = sub.add_parser("seal-day")
    seal.add_argument("--date", required=True)
    evaluate_parser = sub.add_parser("evaluate")
    evaluate_parser.add_argument("--as-of")
    sub.add_parser("verify")
    run = sub.add_parser("run")
    run.add_argument("--duration-seconds", type=int, default=0)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    config = load_json(args.config)
    validate_config(config)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    if args.command == "collect-once":
        sample = create_sample(config, args.output_dir)
        append_sample(args.output_dir, sample)
        print(json.dumps(sample, sort_keys=True))
    elif args.command == "seal-day":
        print(json.dumps(seal_day(args.output_dir, dt.date.fromisoformat(args.date)), sort_keys=True))
    elif args.command == "evaluate":
        as_of = parse_time(args.as_of) if args.as_of else None
        print(json.dumps(evaluate(config, args.output_dir, as_of), sort_keys=True))
    elif args.command == "verify":
        print(json.dumps(verify_records(args.output_dir), sort_keys=True))
    elif args.command == "run":
        if args.duration_seconds < 0:
            raise EvidenceError("duration-seconds must be non-negative")
        return run_loop(config, args.output_dir, args.duration_seconds)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except EvidenceError as error:
        raise SystemExit(f"operational-evidence: {error}") from error
