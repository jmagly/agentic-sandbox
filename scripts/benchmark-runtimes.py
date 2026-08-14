#!/usr/bin/env python3
"""Run the credential-free cross-runtime benchmark required by issue #660.

Each runtime adapter receives the same Python workload on stdin and must emit
one JSON object on stdout. Adapter commands are argv arrays, never shell text.
The evidence records only a digest of the adapter configuration so local paths
and endpoint details do not leak into committed output.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import platform
import re
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SCHEMA = "agentic-sandbox.runtime-benchmark.v1"
RUNTIME_NAMES = ("host", "docker", "qemu-libvirt", "cloud-hypervisor")
FORBIDDEN_CONFIG = re.compile(
    r"(?i)(authorization|bearer[ =]|password[ =]|passwd[ =]|secret[ =]|token[ =]|"
    r"api[_-]?key[ =]|client[_-]?secret[ =]|https?://[^/\s:@]+:[^/\s@]+@)"
)

WORKLOAD = r'''import hashlib,json,os,resource,sys,tempfile,time
cfg=json.loads(AGENTIC_RUNTIME_BENCHMARK_WORKLOAD)

cpu_started_wall=time.perf_counter()
cpu_started_process=time.process_time()
digest=b"agentic-sandbox-runtime-benchmark"
for index in range(cfg["cpu_iterations"]):
    digest=hashlib.sha256(digest+index.to_bytes(8,"little")).digest()
cpu_process_seconds=time.process_time()-cpu_started_process
cpu_wall_seconds=time.perf_counter()-cpu_started_wall

block=(b"agentic-sandbox-runtime-benchmark\n"*32768)[:1048576]
with tempfile.TemporaryDirectory(prefix="agentic-runtime-benchmark-") as directory:
    path=os.path.join(directory,"io.bin")
    write_started=time.perf_counter()
    with open(path,"wb",buffering=0) as handle:
        for _ in range(cfg["io_mib"]):
            handle.write(block)
        fsync_started=time.perf_counter()
        os.fsync(handle.fileno())
        fsync_seconds=time.perf_counter()-fsync_started
    write_seconds=time.perf_counter()-write_started
    read_started=time.perf_counter()
    read_digest=hashlib.sha256()
    with open(path,"rb",buffering=0) as handle:
        while True:
            chunk=handle.read(1048576)
            if not chunk:
                break
            read_digest.update(chunk)
    read_seconds=time.perf_counter()-read_started

task_started=time.perf_counter()
task_digest=hashlib.sha256()
for index in range(cfg["task_iterations"]):
    payload=json.dumps({"command":"echo","index":index,"runtime":"benchmark"},
                       separators=(",",":"),sort_keys=True).encode()
    task_digest.update(hashlib.sha256(payload).digest())
task_seconds=time.perf_counter()-task_started

rss=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
if sys.platform == "darwin":
    rss //= 1024
print(json.dumps({
    "schema":"agentic-sandbox.runtime-workload.v1",
    "cpu":{"wall_seconds":cpu_wall_seconds,"process_seconds":cpu_process_seconds,
           "digest":digest.hex()},
    "memory":{"max_rss_kib":int(rss)},
    "io":{"mib":cfg["io_mib"],"write_seconds":write_seconds,
          "read_seconds":read_seconds,"fsync_seconds":fsync_seconds,
          "digest":read_digest.hexdigest()},
    "task":{"iterations":cfg["task_iterations"],"seconds":task_seconds,
            "digest":task_digest.hexdigest()}
},separators=(",",":"),sort_keys=True))
'''


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def canonical_digest(value: Any) -> str:
    encoded = json.dumps(value, separators=(",", ":"), sort_keys=True).encode()
    return hashlib.sha256(encoded).hexdigest()


def load_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected a JSON object")
    return value


def _validate_argv(value: Any, label: str) -> None:
    if not isinstance(value, list) or not value:
        raise ValueError(f"{label} must be a non-empty argv array")
    for arg in value:
        if not isinstance(arg, str) or not arg or "\0" in arg or "\n" in arg:
            raise ValueError(f"{label} entries must be non-empty single-line strings")
        if FORBIDDEN_CONFIG.search(arg):
            raise ValueError(f"{label} contains credential-bearing material")


def validate_config(config: dict[str, Any]) -> None:
    if config.get("schema") != SCHEMA:
        raise ValueError(f"config schema must be {SCHEMA}")
    if not isinstance(config.get("samples"), int) or config["samples"] < 3:
        raise ValueError("samples must be at least 3")
    if not isinstance(config.get("warmups"), int) or config["warmups"] < 0:
        raise ValueError("warmups must be a non-negative integer")
    if not isinstance(config.get("timeout_seconds"), int) or config["timeout_seconds"] < 1:
        raise ValueError("timeout_seconds must be a positive integer")
    workload = config.get("workload")
    if not isinstance(workload, dict):
        raise ValueError("workload must be an object")
    for field in ("cpu_iterations", "io_mib", "task_iterations"):
        if not isinstance(workload.get(field), int) or workload[field] < 1:
            raise ValueError(f"workload.{field} must be a positive integer")

    adapters = config.get("runtimes")
    if not isinstance(adapters, list) or len(adapters) != len(RUNTIME_NAMES):
        raise ValueError("runtimes must define host, docker, qemu-libvirt, and cloud-hypervisor")
    by_name = {adapter.get("name"): adapter for adapter in adapters if isinstance(adapter, dict)}
    if set(by_name) != set(RUNTIME_NAMES):
        raise ValueError("runtime names must be host, docker, qemu-libvirt, and cloud-hypervisor")
    for name in RUNTIME_NAMES:
        adapter = by_name[name]
        unknown = set(adapter) - {
            "name",
            "command",
            "prepare_command",
            "cleanup_command",
            "not_run_reason",
        }
        if unknown:
            raise ValueError(f"runtime {name} has unknown fields: {sorted(unknown)}")
        reason = adapter.get("not_run_reason")
        command = adapter.get("command")
        if reason is not None:
            if command is not None or not isinstance(reason, str) or not reason.strip():
                raise ValueError(f"runtime {name} NOT RUN must have only a non-empty reason")
            if FORBIDDEN_CONFIG.search(reason):
                raise ValueError(f"runtime {name} NOT RUN reason contains credential material")
            continue
        _validate_argv(command, f"runtime {name} command")
        for field in ("prepare_command", "cleanup_command"):
            if field in adapter:
                _validate_argv(adapter[field], f"runtime {name} {field}")


def percentile(values: list[float], quantile: float) -> float:
    if not values:
        raise ValueError("cannot calculate a percentile without samples")
    ordered = sorted(values)
    rank = max(0, math.ceil(quantile * len(ordered)) - 1)
    return ordered[rank]


def _run_command(
    argv: list[str],
    timeout: int,
    *,
    stdin: str | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        input=stdin,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        check=False,
        env=env,
    )


def run_sample(
    adapter: dict[str, Any], workload: dict[str, int], timeout: int, sample: int
) -> dict[str, Any]:
    encoded_workload = json.dumps(workload, separators=(",", ":"), sort_keys=True)
    source = f"AGENTIC_RUNTIME_BENCHMARK_WORKLOAD={encoded_workload!r}\n{WORKLOAD}"
    started = time.perf_counter()
    result = _run_command(adapter["command"], timeout, stdin=source)
    end_to_end = time.perf_counter() - started
    if result.returncode != 0:
        raise RuntimeError(
            f"runtime {adapter['name']} sample {sample} failed with exit {result.returncode}"
        )
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(
            f"runtime {adapter['name']} sample {sample} did not emit valid JSON"
        ) from error
    if not isinstance(payload, dict) or payload.get("schema") != "agentic-sandbox.runtime-workload.v1":
        raise RuntimeError(f"runtime {adapter['name']} sample {sample} emitted the wrong schema")
    inside = (
        float(payload["cpu"]["wall_seconds"])
        + float(payload["io"]["write_seconds"])
        + float(payload["io"]["read_seconds"])
        + float(payload["task"]["seconds"])
    )
    io_mib = int(payload["io"]["mib"])
    task_iterations = int(payload["task"]["iterations"])
    return {
        "runtime": adapter["name"],
        "sample": sample,
        "end_to_end_seconds": end_to_end,
        "command_overhead_seconds": max(0.0, end_to_end - inside),
        "cpu_wall_seconds": float(payload["cpu"]["wall_seconds"]),
        "cpu_process_seconds": float(payload["cpu"]["process_seconds"]),
        "max_rss_kib": int(payload["memory"]["max_rss_kib"]),
        "io_write_mib_s": io_mib / float(payload["io"]["write_seconds"]),
        "io_read_mib_s": io_mib / float(payload["io"]["read_seconds"]),
        "io_fsync_ms": float(payload["io"]["fsync_seconds"]) * 1000,
        "task_throughput_s": task_iterations / float(payload["task"]["seconds"]),
        "task_completion_ms": float(payload["task"]["seconds"]) * 1000,
        "cpu_digest": payload["cpu"]["digest"],
        "io_digest": payload["io"]["digest"],
        "task_digest": payload["task"]["digest"],
    }


def summarize(samples: list[dict[str, Any]], not_run: dict[str, str]) -> dict[str, Any]:
    rows: list[dict[str, Any]] = []
    grouped = {
        name: [sample for sample in samples if sample["runtime"] == name]
        for name in RUNTIME_NAMES
    }
    host = grouped["host"]
    if not host:
        raise ValueError("host baseline must run")
    host_cpu = statistics.median(row["cpu_wall_seconds"] for row in host)
    host_write = statistics.median(row["io_write_mib_s"] for row in host)
    host_read = statistics.median(row["io_read_mib_s"] for row in host)
    for name in RUNTIME_NAMES:
        runtime_samples = grouped[name]
        if not runtime_samples:
            rows.append({"runtime": name, "status": "NOT RUN", "reason": not_run[name]})
            continue
        cpu = [row["cpu_wall_seconds"] for row in runtime_samples]
        write = [row["io_write_mib_s"] for row in runtime_samples]
        read = [row["io_read_mib_s"] for row in runtime_samples]
        launch = [row["command_overhead_seconds"] * 1000 for row in runtime_samples]
        task = [row["task_completion_ms"] for row in runtime_samples]
        cpu_median = statistics.median(cpu)
        write_median = statistics.median(write)
        read_median = statistics.median(read)
        rows.append(
            {
                "runtime": name,
                "status": "measured",
                "samples": len(runtime_samples),
                "command_overhead_ms": {
                    "p50": statistics.median(launch),
                    "p95": percentile(launch, 0.95),
                },
                "cpu_workload_seconds": {
                    "p50": cpu_median,
                    "p95": percentile(cpu, 0.95),
                    "overhead_vs_host_pct": ((cpu_median / host_cpu) - 1) * 100,
                },
                "memory_max_rss_kib": {
                    "p50": statistics.median(row["max_rss_kib"] for row in runtime_samples),
                    "p95": percentile([row["max_rss_kib"] for row in runtime_samples], 0.95),
                },
                "io_write_mib_s": {
                    "p50": write_median,
                    "p95": percentile(write, 0.95),
                    "pct_of_host": (write_median / host_write) * 100,
                },
                "io_read_mib_s": {
                    "p50": read_median,
                    "p95": percentile(read, 0.95),
                    "pct_of_host": (read_median / host_read) * 100,
                },
                "task_completion_ms": {
                    "p50": statistics.median(task),
                    "p95": percentile(task, 0.95),
                },
                "task_throughput_s": {
                    "p50": statistics.median(row["task_throughput_s"] for row in runtime_samples),
                    "p95": percentile([row["task_throughput_s"] for row in runtime_samples], 0.95),
                },
            }
        )
    return {"schema": SCHEMA, "runtimes": rows}


def sanitized_environment() -> dict[str, Any]:
    memory_kib = None
    try:
        with Path("/proc/meminfo").open(encoding="ascii") as handle:
            for line in handle:
                if line.startswith("MemTotal:"):
                    memory_kib = int(line.split()[1])
                    break
    except (OSError, ValueError):
        pass
    return {
        "os": platform.system().lower(),
        "architecture": platform.machine().lower(),
        "kernel": ".".join(platform.release().split(".")[:2]),
        "logical_cpus": os.cpu_count(),
        "memory_gib": round(memory_kib / 1024 / 1024, 1) if memory_kib else None,
        "python": platform.python_version(),
    }


def write_evidence(
    output: Path,
    config: dict[str, Any],
    samples: list[dict[str, Any]],
    launch: dict[str, Any],
    not_run: dict[str, str],
) -> None:
    output.mkdir(parents=True, exist_ok=True)
    summary = summarize(samples, not_run)
    commit_result = subprocess.run(
        ["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=False
    )
    dirty_result = subprocess.run(
        ["git", "status", "--porcelain"], capture_output=True, text=True, check=False
    )
    metadata = {
        "schema": SCHEMA,
        "generated_at": utc_now(),
        "implementation_commit": commit_result.stdout.strip() or "unknown",
        "implementation_worktree_dirty": bool(dirty_result.stdout.strip()),
        "benchmark_runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "environment": sanitized_environment(),
        "config_sha256": canonical_digest(config),
        "samples_per_runtime": config["samples"],
        "warmups": config["warmups"],
        "workload": config["workload"],
        "launch_to_ready": launch,
        "evidence_limits": [
            "command overhead is not equivalent to full cold provision-to-ready latency unless an adapter prepare_command is configured",
            "workload process RSS does not include runtime daemon or VMM resident memory",
            "three samples support p50/p95 comparison but not a high-confidence tail model",
        ],
    }
    raw = {**metadata, "samples": samples}
    with (output / "raw.json").open("w", encoding="utf-8") as handle:
        json.dump(raw, handle, indent=2, sort_keys=True)
        handle.write("\n")
    with (output / "summary.json").open("w", encoding="utf-8") as handle:
        json.dump({**metadata, **summary}, handle, indent=2, sort_keys=True)
        handle.write("\n")
    fields = list(samples[0])
    with (output / "samples.csv").open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        writer.writerows(samples)

    lines = [
        "# Runtime benchmark report",
        "",
        f"Generated: `{metadata['generated_at']}`  ",
        f"Implementation: `{metadata['implementation_commit']}`  ",
        f"Implementation worktree dirty: `{str(metadata['implementation_worktree_dirty']).lower()}`  ",
        f"Benchmark runner SHA-256: `{metadata['benchmark_runner_sha256']}`  ",
        f"Config digest: `{metadata['config_sha256']}`",
        "",
        "| Runtime | Status | Samples | Command/launch p50 | CPU overhead vs host | Write vs host | Read vs host |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for row in summary["runtimes"]:
        if row["status"] == "NOT RUN":
            lines.append(f"| {row['runtime']} | NOT RUN: {row['reason']} | - | - | - | - | - |")
        else:
            lines.append(
                "| {runtime} | measured | {samples} | {launch:.2f} ms | {cpu:.2f}% | {write:.2f}% | {read:.2f}% |".format(
                    runtime=row["runtime"],
                    samples=row["samples"],
                    launch=row["command_overhead_ms"]["p50"],
                    cpu=row["cpu_workload_seconds"]["overhead_vs_host_pct"],
                    write=row["io_write_mib_s"]["pct_of_host"],
                    read=row["io_read_mib_s"]["pct_of_host"],
                )
            )
    lines.extend(
        [
            "",
            "## Evidence limits",
            "",
            *[f"- {item}" for item in metadata["evidence_limits"]],
            "",
            "Raw measurements are in `raw.json` and `samples.csv`; machine-readable aggregates are in `summary.json`.",
        ]
    )
    (output / "REPORT.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def run(config: dict[str, Any], output: Path) -> None:
    validate_config(config)
    adapters = {adapter["name"]: adapter for adapter in config["runtimes"]}
    samples: list[dict[str, Any]] = []
    launch: dict[str, Any] = {}
    not_run: dict[str, str] = {}
    prepared: list[dict[str, Any]] = []
    try:
        for name in RUNTIME_NAMES:
            adapter = adapters[name]
            if "not_run_reason" in adapter:
                not_run[name] = adapter["not_run_reason"]
                launch[name] = {"status": "NOT RUN", "reason": adapter["not_run_reason"]}
                continue
            if "prepare_command" in adapter:
                if "cleanup_command" in adapter:
                    prepared.append(adapter)
                started = time.perf_counter()
                result = _run_command(adapter["prepare_command"], config["timeout_seconds"])
                elapsed = time.perf_counter() - started
                if result.returncode != 0:
                    raise RuntimeError(f"runtime {name} prepare command failed with exit {result.returncode}")
                launch[name] = {"status": "measured", "seconds": elapsed}
            else:
                launch[name] = {"status": "not-separated", "reason": "adapter has no prepare_command"}
            for warmup in range(config["warmups"]):
                run_sample(adapter, config["workload"], config["timeout_seconds"], -(warmup + 1))
            for sample in range(1, config["samples"] + 1):
                samples.append(
                    run_sample(adapter, config["workload"], config["timeout_seconds"], sample)
                )
    finally:
        for adapter in reversed(prepared):
            if "cleanup_command" in adapter:
                result = _run_command(adapter["cleanup_command"], config["timeout_seconds"])
                if result.returncode != 0:
                    print(
                        f"warning: runtime {adapter['name']} cleanup failed with exit {result.returncode}",
                        file=sys.stderr,
                    )
    if not samples:
        raise RuntimeError("no runtime samples were collected")
    write_evidence(output, config, samples, launch, not_run)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    try:
        run(load_json(args.config), args.output)
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"runtime benchmark failed: {error}", file=sys.stderr)
        return 1
    print(f"runtime benchmark evidence written to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
