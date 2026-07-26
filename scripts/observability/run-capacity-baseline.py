#!/usr/bin/env python3
"""Credential-free seven-day capacity load and evidence collector.

The harness targets only pre-provisioned synthetic agents in an explicitly
approved isolated environment. It retains status, latency, state, and numeric
Prometheus values; it never retains response bodies, environment dumps,
terminal output, hostnames, addresses, credentials, or agent metadata.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import hashlib
import json
import math
import os
import signal
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

SCHEMA = "agentic-sandbox.capacity-baseline.v1"
STOP = False


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def load_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected a JSON object")
    return value


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_config(config: dict[str, Any]) -> None:
    if config.get("schema") != SCHEMA:
        raise ValueError(f"config schema must be {SCHEMA}")
    for field in ("duration_seconds", "interval_seconds", "request_timeout_seconds"):
        if not isinstance(config.get(field), int) or config[field] <= 0:
            raise ValueError(f"{field} must be a positive integer")
    if config["duration_seconds"] < 604800:
        raise ValueError("duration_seconds must cover at least seven days (604800 seconds)")
    runtimes = {agent.get("runtime") for agent in config.get("agents", [])}
    if runtimes != {"host", "docker", "qemu"}:
        raise ValueError("agents must cover exactly the host, docker, and qemu runtime mix")
    for agent in config["agents"]:
        identifier = agent.get("id", "")
        if not isinstance(identifier, str) or not identifier.startswith("capacity-"):
            raise ValueError("every synthetic agent id must start with 'capacity-'")
    for field in ("management_url", "prometheus_url"):
        parsed = urllib.parse.urlparse(config.get(field, ""))
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError(f"{field} must be an HTTP(S) URL")
        if parsed.username or parsed.password or parsed.query or parsed.fragment:
            raise ValueError(f"{field} must not contain credentials, query data, or fragments")


def request_json(
    method: str,
    url: str,
    timeout: int,
    payload: dict[str, Any] | None = None,
) -> tuple[int, dict[str, Any] | None, float]:
    body = None
    headers = {"Accept": "application/json"}
    if payload is not None:
        body = json.dumps(payload, separators=(",", ":")).encode()
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=body, headers=headers, method=method)
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status = response.status
            raw = response.read(1024 * 1024)
    except urllib.error.HTTPError as error:
        status = error.code
        raw = error.read(1024 * 1024)
    latency_ms = (time.monotonic() - started) * 1000
    parsed = None
    if raw:
        try:
            value = json.loads(raw)
            if isinstance(value, dict):
                parsed = value
        except (UnicodeDecodeError, json.JSONDecodeError):
            pass
    return status, parsed, latency_ms


def event(
    operation: str,
    *,
    runtime: str = "control",
    status: int = 0,
    latency_ms: float = 0.0,
    outcome: str = "success",
    state: str | None = None,
    value: float | None = None,
) -> dict[str, Any]:
    result: dict[str, Any] = {
        "ts": utc_now(),
        "operation": operation,
        "runtime": runtime,
        "status": status,
        "latency_ms": round(latency_ms, 3),
        "outcome": outcome,
    }
    if state is not None:
        result["state"] = state
    if value is not None and math.isfinite(value):
        result["value"] = value
    return result


def health_event(config: dict[str, Any]) -> dict[str, Any]:
    try:
        status, _, latency = request_json(
            "GET",
            f"{config['management_url']}/healthz",
            config["request_timeout_seconds"],
        )
        return event(
            "management_health",
            status=status,
            latency_ms=latency,
            outcome="success" if 200 <= status < 300 else "error",
        )
    except (OSError, TimeoutError) as error:
        return event("management_health", outcome=type(error).__name__.lower())


def task_event(config: dict[str, Any], agent: dict[str, str]) -> dict[str, Any]:
    payload = {
        "message": {
            "role": "user",
            "parts": [{"kind": "text", "text": "capacity-baseline-synthetic-echo"}],
        }
    }
    started = time.monotonic()
    try:
        status, response, _ = request_json(
            "POST",
            f"{config['management_url']}/agents/{urllib.parse.quote(agent['id'])}/v1/messages:send",
            config["request_timeout_seconds"],
            payload,
        )
        task_id = (response or {}).get("id")
        if not 200 <= status < 300 or not isinstance(task_id, str):
            return event(
                "task",
                runtime=agent["runtime"],
                status=status,
                latency_ms=(time.monotonic() - started) * 1000,
                outcome="error",
            )
        deadline = time.monotonic() + config.get("task_timeout_seconds", 120)
        state = "timeout"
        while time.monotonic() < deadline:
            poll_status, poll, _ = request_json(
                "GET",
                f"{config['management_url']}/agents/{urllib.parse.quote(agent['id'])}/v1/tasks/{urllib.parse.quote(task_id)}",
                config["request_timeout_seconds"],
            )
            state = str((poll or {}).get("status", {}).get("state", "unknown"))
            if state in {"completed", "failed", "canceled", "rejected"}:
                status = poll_status
                break
            time.sleep(0.25)
        return event(
            "task",
            runtime=agent["runtime"],
            status=status,
            latency_ms=(time.monotonic() - started) * 1000,
            outcome="success" if state == "completed" else state,
            state=state,
        )
    except (OSError, TimeoutError) as error:
        return event(
            "task",
            runtime=agent["runtime"],
            latency_ms=(time.monotonic() - started) * 1000,
            outcome=type(error).__name__.lower(),
        )


def session_event(config: dict[str, Any], agent: dict[str, str]) -> dict[str, Any]:
    payload = {
        "command": "sh",
        "args": ["-c", f"sleep {config.get('session_hold_seconds', 5)}"],
        "working_dir": "/tmp",
        "session_name": "capacity-baseline-synthetic",
    }
    started = time.monotonic()
    status = 0
    state = "create_failed"
    try:
        status, response, _ = request_json(
            "POST",
            f"{config['management_url']}/api/v1/agents/{urllib.parse.quote(agent['id'])}/sessions",
            config["request_timeout_seconds"],
            payload,
        )
        session_id = (response or {}).get("session_id")
        if 200 <= status < 300 and isinstance(session_id, str):
            state = "created"
            delete_status, _, _ = request_json(
                "DELETE",
                f"{config['management_url']}/api/v1/agents/{urllib.parse.quote(agent['id'])}/sessions/{urllib.parse.quote(session_id)}",
                config["request_timeout_seconds"],
            )
            status = delete_status
            state = "cleaned" if 200 <= delete_status < 300 else "cleanup_failed"
        return event(
            "session",
            runtime=agent["runtime"],
            status=status,
            latency_ms=(time.monotonic() - started) * 1000,
            outcome="success" if state == "cleaned" else "error",
            state=state,
        )
    except (OSError, TimeoutError) as error:
        return event(
            "session",
            runtime=agent["runtime"],
            latency_ms=(time.monotonic() - started) * 1000,
            outcome=type(error).__name__.lower(),
            state=state,
        )


def lifecycle_event(config: dict[str, Any], agent: dict[str, str]) -> dict[str, Any]:
    started = time.monotonic()
    status = 0
    state = "stop_failed"
    try:
        status, _, _ = request_json(
            "POST",
            f"{config['management_url']}/api/v2/admin/instances/{urllib.parse.quote(agent['id'])}/stop",
            config["request_timeout_seconds"],
        )
        if 200 <= status < 300:
            state = "stopped"
            status, _, _ = request_json(
                "POST",
                f"{config['management_url']}/api/v2/admin/instances/{urllib.parse.quote(agent['id'])}/start",
                config["request_timeout_seconds"],
            )
            state = "restarted" if 200 <= status < 300 else "restart_failed"
        return event(
            "lifecycle",
            runtime=agent["runtime"],
            status=status,
            latency_ms=(time.monotonic() - started) * 1000,
            outcome="success" if state == "restarted" else "error",
            state=state,
        )
    except (OSError, TimeoutError) as error:
        return event(
            "lifecycle",
            runtime=agent["runtime"],
            latency_ms=(time.monotonic() - started) * 1000,
            outcome=type(error).__name__.lower(),
            state=state,
        )


def prometheus_events(config: dict[str, Any]) -> list[dict[str, Any]]:
    results = []
    for name, query in config.get("prometheus_queries", {}).items():
        url = f"{config['prometheus_url']}/api/v1/query?{urllib.parse.urlencode({'query': query})}"
        started = time.monotonic()
        try:
            status, response, latency = request_json(
                "GET", url, config["request_timeout_seconds"]
            )
            values = []
            for item in ((response or {}).get("data", {}).get("result", [])):
                raw = item.get("value", [None, None])[1]
                try:
                    values.append(float(raw))
                except (TypeError, ValueError):
                    continue
            value = statistics.fmean(values) if values else None
            results.append(
                event(
                    f"prometheus:{name}",
                    status=status,
                    latency_ms=latency,
                    outcome="success" if 200 <= status < 300 and values else "no_data",
                    value=value,
                )
            )
        except (OSError, TimeoutError) as error:
            results.append(
                event(
                    f"prometheus:{name}",
                    latency_ms=(time.monotonic() - started) * 1000,
                    outcome=type(error).__name__.lower(),
                )
            )
    return results


def append_events(path: Path, events: list[dict[str, Any]]) -> None:
    with path.open("a", encoding="utf-8") as handle:
        for item in events:
            handle.write(json.dumps(item, sort_keys=True, separators=(",", ":")) + "\n")
        handle.flush()
        os.fsync(handle.fileno())


def git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"], text=True, stderr=subprocess.DEVNULL
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def run(config_path: Path, output_dir: Path, environment_name: str) -> int:
    config = load_json(config_path)
    validate_config(config)
    output_dir.mkdir(parents=True, exist_ok=True)
    events_path = output_dir / "events.jsonl"
    manifest_path = output_dir / "manifest.json"
    interruptions_path = output_dir / "interruptions.jsonl"

    if manifest_path.exists():
        manifest = load_json(manifest_path)
        if manifest.get("config_sha256") != sha256(config_path):
            raise ValueError("resume refused: config digest differs from existing manifest")
    else:
        manifest = {
            "schema": SCHEMA,
            "started_at": utc_now(),
            "completed_at": None,
            "active_duration_seconds": 0.0,
            "implementation_commit": git_commit(),
            "config_sha256": sha256(config_path),
            "environment_profile": environment_name,
            "approved_isolated_environment": True,
            "expected_duration_seconds": config["duration_seconds"],
            "runtime_mix": sorted(agent["runtime"] for agent in config["agents"]),
        }
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")

    active_before = float(manifest.get("active_duration_seconds", 0.0))
    remaining = config["duration_seconds"] - active_before
    run_started = time.monotonic()
    deadline = run_started + max(0.0, remaining)
    interval = 0
    while time.monotonic() < deadline and not STOP:
        cycle_started = time.monotonic()
        items = [health_event(config)]
        with concurrent.futures.ThreadPoolExecutor(max_workers=len(config["agents"]) * 2) as pool:
            futures = []
            for agent in config["agents"]:
                futures.append(pool.submit(task_event, config, agent))
                futures.append(pool.submit(session_event, config, agent))
            items.extend(future.result() for future in futures)
        if interval > 0 and interval % config.get("lifecycle_every_intervals", 60) == 0:
            with concurrent.futures.ThreadPoolExecutor(max_workers=len(config["agents"])) as pool:
                items.extend(
                    pool.submit(lifecycle_event, config, agent).result()
                    for agent in config["agents"]
                )
        items.extend(prometheus_events(config))
        append_events(events_path, items)
        interval += 1
        sleep_for = config["interval_seconds"] - (time.monotonic() - cycle_started)
        if sleep_for > 0:
            time.sleep(sleep_for)

    manifest["active_duration_seconds"] = round(
        active_before + (time.monotonic() - run_started), 3
    )
    if STOP:
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        append_events(
            interruptions_path,
            [event("interruption", outcome="signal", state="resume-required")],
        )
        return 2
    manifest["completed_at"] = utc_now()
    manifest["actual_duration_seconds"] = manifest["active_duration_seconds"]
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    summarize(output_dir)
    return 0


def percentile(values: list[float], quantile: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = min(len(ordered) - 1, math.ceil(quantile * len(ordered)) - 1)
    return round(ordered[index], 3)


def summarize(output_dir: Path) -> None:
    events_path = output_dir / "events.jsonl"
    manifest = load_json(output_dir / "manifest.json")
    grouped: dict[tuple[str, str], list[dict[str, Any]]] = {}
    with events_path.open(encoding="utf-8") as handle:
        for line in handle:
            item = json.loads(line)
            grouped.setdefault((item["operation"], item["runtime"]), []).append(item)
    rows = []
    for (operation, runtime), items in sorted(grouped.items()):
        latencies = [float(item["latency_ms"]) for item in items]
        success = sum(item["outcome"] == "success" for item in items)
        values = [float(item["value"]) for item in items if "value" in item]
        rows.append(
            {
                "operation": operation,
                "runtime": runtime,
                "samples": len(items),
                "success_rate": round(success / len(items), 6),
                "latency_ms": {
                    "p50": percentile(latencies, 0.50),
                    "p95": percentile(latencies, 0.95),
                    "p99": percentile(latencies, 0.99),
                },
                "value": {
                    "min": min(values) if values else None,
                    "mean": round(statistics.fmean(values), 6) if values else None,
                    "max": max(values) if values else None,
                },
            }
        )
    summary = {
        "schema": SCHEMA,
        "generated_at": utc_now(),
        "manifest": manifest,
        "complete_seven_day_window": bool(manifest.get("completed_at"))
        and manifest.get("actual_duration_seconds", 0) >= 604800,
        "series": rows,
    }
    (output_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def preflight(config_path: Path) -> int:
    config = load_json(config_path)
    validate_config(config)
    checks = [health_event(config), *prometheus_events(config)]
    print(json.dumps(checks, indent=2, sort_keys=True))
    return 0 if all(item["outcome"] in {"success", "no_data"} for item in checks) else 1


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, required=True)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("preflight")
    run_parser = subparsers.add_parser("run")
    run_parser.add_argument("--output-dir", type=Path, required=True)
    run_parser.add_argument("--approved-isolated-environment", required=True)
    summary_parser = subparsers.add_parser("summarize")
    summary_parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "preflight":
        return preflight(args.config)
    if args.command == "run":
        return run(args.config, args.output_dir, args.approved_isolated_environment)
    summarize(args.output_dir)
    return 0


def handle_signal(_signum: int, _frame: Any) -> None:
    global STOP
    STOP = True


if __name__ == "__main__":
    signal.signal(signal.SIGINT, handle_signal)
    signal.signal(signal.SIGTERM, handle_signal)
    try:
        raise SystemExit(main())
    except (ValueError, OSError, json.JSONDecodeError) as error:
        print(f"capacity baseline error: {error}", file=sys.stderr)
        raise SystemExit(1)
