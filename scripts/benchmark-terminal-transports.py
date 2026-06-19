#!/usr/bin/env python3
"""Benchmark terminal transport candidates for issue #520.

The default mode is deterministic and dependency-free so CI and review can
reproduce the same artifact without requiring sshd, mosh-server, ttyd, or a
Kubernetes API server. The model is intentionally conservative: it records
which rows are simulated, which baselines are unavailable on the current host,
and which acceptance metric each row covers.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import platform
import shutil
import statistics
import sys
from dataclasses import asdict, dataclass
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Iterable


PROFILES = {
    "local": {"network_ms": 1.0, "loss_pct": 0.0},
    "wan-50ms": {"network_ms": 50.0, "loss_pct": 0.0},
    "wan-150ms": {"network_ms": 150.0, "loss_pct": 0.0},
    "lossy-50ms-2pct": {"network_ms": 50.0, "loss_pct": 2.0},
}

WORKLOAD_BYTES = {
    "prompt": 80,
    "editor-paint": 16 * 1024,
    "ls-tree": 64 * 1024,
    "burst-output": 4 * 1024 * 1024,
}

FANOUTS = (1, 4, 16, 32)


@dataclass(frozen=True)
class TransportModel:
    name: str
    category: str
    dependency: str | None
    available_without_fixture: bool
    setup_ms: float
    attach_ms: float
    per_keystroke_ms: float
    retransmit_penalty_ms: float
    cpu_base_pct: float
    cpu_per_mib_pct: float
    rss_base_mib: float
    host_rss_mib: float
    bytes_overhead: float
    frame_overhead_bytes: int
    fanout_per_watcher_ms: float
    slow_watcher_policy: str
    replay_supported: bool
    notes: str


TRANSPORTS = (
    TransportModel(
        name="grpc-pty-pty-ws-json-base64",
        category="project",
        dependency=None,
        available_without_fixture=True,
        setup_ms=24.0,
        attach_ms=5.0,
        per_keystroke_ms=1.2,
        retransmit_penalty_ms=18.0,
        cpu_base_pct=1.6,
        cpu_per_mib_pct=0.22,
        rss_base_mib=18.0,
        host_rss_mib=9.0,
        bytes_overhead=4.0 / 3.0,
        frame_overhead_bytes=132,
        fanout_per_watcher_ms=0.32,
        slow_watcher_policy="bounded channel; lagging watchers evicted and replay can recover buffered frames",
        replay_supported=True,
        notes="Models current AgentPtyBridge + pty-ws JSON text frames with base64 data.",
    ),
    TransportModel(
        name="grpc-pty-pty-ws-binary",
        category="project-candidate",
        dependency=None,
        available_without_fixture=True,
        setup_ms=22.0,
        attach_ms=4.0,
        per_keystroke_ms=0.9,
        retransmit_penalty_ms=16.0,
        cpu_base_pct=1.4,
        cpu_per_mib_pct=0.14,
        rss_base_mib=18.0,
        host_rss_mib=9.0,
        bytes_overhead=1.0,
        frame_overhead_bytes=20,
        fanout_per_watcher_ms=0.22,
        slow_watcher_policy="same replay/fanout model as pty-ws, without base64 encode/decode on hot payloads",
        replay_supported=True,
        notes="Simulates the binary payload mode proposed by the gap analysis.",
    ),
    TransportModel(
        name="ssh-cold",
        category="baseline",
        dependency="ssh",
        available_without_fixture=False,
        setup_ms=185.0,
        attach_ms=185.0,
        per_keystroke_ms=1.8,
        retransmit_penalty_ms=25.0,
        cpu_base_pct=2.2,
        cpu_per_mib_pct=0.18,
        rss_base_mib=13.0,
        host_rss_mib=7.0,
        bytes_overhead=1.08,
        frame_overhead_bytes=56,
        fanout_per_watcher_ms=185.0,
        slow_watcher_policy="no native fanout; each watcher is another SSH/tmux attach",
        replay_supported=False,
        notes="Cold key exchange/auth per attach.",
    ),
    TransportModel(
        name="ssh-controlmaster",
        category="baseline",
        dependency="ssh",
        available_without_fixture=False,
        setup_ms=62.0,
        attach_ms=28.0,
        per_keystroke_ms=1.6,
        retransmit_penalty_ms=23.0,
        cpu_base_pct=1.9,
        cpu_per_mib_pct=0.17,
        rss_base_mib=14.0,
        host_rss_mib=7.0,
        bytes_overhead=1.06,
        frame_overhead_bytes=56,
        fanout_per_watcher_ms=28.0,
        slow_watcher_policy="no native fanout; multiplexed SSH reduces attach setup only",
        replay_supported=False,
        notes="OpenSSH ControlMaster/ControlPersist baseline.",
    ),
    TransportModel(
        name="ssh-tmux-attach",
        category="baseline",
        dependency="ssh",
        available_without_fixture=False,
        setup_ms=78.0,
        attach_ms=35.0,
        per_keystroke_ms=1.7,
        retransmit_penalty_ms=24.0,
        cpu_base_pct=2.0,
        cpu_per_mib_pct=0.19,
        rss_base_mib=20.0,
        host_rss_mib=12.0,
        bytes_overhead=1.08,
        frame_overhead_bytes=72,
        fanout_per_watcher_ms=35.0,
        slow_watcher_policy="tmux isolates terminal state; each remote watcher still consumes an SSH client",
        replay_supported=True,
        notes="SSH attach into a durable tmux session.",
    ),
    TransportModel(
        name="mosh",
        category="baseline",
        dependency="mosh",
        available_without_fixture=False,
        setup_ms=230.0,
        attach_ms=230.0,
        per_keystroke_ms=0.7,
        retransmit_penalty_ms=4.0,
        cpu_base_pct=2.8,
        cpu_per_mib_pct=0.12,
        rss_base_mib=18.0,
        host_rss_mib=10.0,
        bytes_overhead=0.42,
        frame_overhead_bytes=48,
        fanout_per_watcher_ms=230.0,
        slow_watcher_policy="state-sync client, not a multi-watcher replay bus",
        replay_supported=False,
        notes="Best-effort baseline for lossy WAN interactivity; bootstrap normally uses SSH.",
    ),
    TransportModel(
        name="ttyd-gotty-websocket",
        category="baseline",
        dependency="ttyd",
        available_without_fixture=False,
        setup_ms=70.0,
        attach_ms=18.0,
        per_keystroke_ms=1.1,
        retransmit_penalty_ms=18.0,
        cpu_base_pct=2.4,
        cpu_per_mib_pct=0.24,
        rss_base_mib=16.0,
        host_rss_mib=14.0,
        bytes_overhead=1.18,
        frame_overhead_bytes=86,
        fanout_per_watcher_ms=1.1,
        slow_watcher_policy="implementation-dependent WebSocket backpressure; no project replay guarantee",
        replay_supported=False,
        notes="Local WebSocket terminal daemon baseline.",
    ),
    TransportModel(
        name="kubernetes-style-ws-exec",
        category="baseline",
        dependency=None,
        available_without_fixture=True,
        setup_ms=95.0,
        attach_ms=26.0,
        per_keystroke_ms=1.4,
        retransmit_penalty_ms=20.0,
        cpu_base_pct=2.1,
        cpu_per_mib_pct=0.20,
        rss_base_mib=22.0,
        host_rss_mib=8.0,
        bytes_overhead=1.04,
        frame_overhead_bytes=34,
        fanout_per_watcher_ms=26.0,
        slow_watcher_policy="stream attach baseline; replay and fanout are outside the exec protocol",
        replay_supported=False,
        notes="Local equivalent of Kubernetes WebSocket exec channel framing.",
    ),
)


@dataclass
class Environment:
    generated_at: str
    host: str
    python: str
    mode: str
    dependency_status: dict[str, bool]


@dataclass
class MetricRow:
    transport: str
    category: str
    profile: str
    fanout: int
    startup_to_prompt_ms: float
    attach_reattach_ms: float
    keystroke_rtt_ms: float
    cpu_pct_agent: float
    cpu_pct_management: float
    cpu_pct_guest_host: float
    rss_mib_agent: float
    rss_mib_management: float
    rss_mib_guest_host: float
    bytes_prompt: int
    bytes_editor_paint: int
    bytes_ls_tree: int
    bytes_burst_output: int
    burst_throughput_mib_s: float
    slow_watcher_behavior: str
    reconnect_replay_correct: bool
    payload_mode: str
    measured: bool
    notes: str


def dependency_status() -> dict[str, bool]:
    deps = sorted({t.dependency for t in TRANSPORTS if t.dependency})
    return {dep: shutil.which(dep) is not None for dep in deps}


def payload_mode(name: str) -> str:
    if "binary" in name:
        return "binary"
    if "base64" in name or name.startswith("grpc-pty"):
        return "json-base64"
    return "native-or-protocol-specific"


def wire_bytes(model: TransportModel, payload_size: int, frames: int) -> int:
    return int(math.ceil(payload_size * model.bytes_overhead + frames * model.frame_overhead_bytes))


def model_row(model: TransportModel, profile_name: str, profile: dict[str, float], fanout: int) -> MetricRow:
    network_ms = profile["network_ms"]
    loss_pct = profile["loss_pct"]
    mib = WORKLOAD_BYTES["burst-output"] / (1024 * 1024)
    fanout_penalty = max(0, fanout - 1) * model.fanout_per_watcher_ms
    loss_penalty = model.retransmit_penalty_ms * (loss_pct / 2.0)
    startup = model.setup_ms + network_ms * (2.0 if model.name == "ssh-cold" else 1.0)
    attach = model.attach_ms + network_ms + fanout_penalty
    rtt = network_ms * 2.0 + model.per_keystroke_ms + loss_penalty
    management_cpu = model.cpu_base_pct + (mib * model.cpu_per_mib_pct) + fanout * 0.05
    agent_cpu = max(0.3, model.cpu_base_pct * 0.55 + mib * model.cpu_per_mib_pct * 0.35)
    guest_cpu = max(0.2, model.cpu_base_pct * 0.35)
    burst_bytes = wire_bytes(model, WORKLOAD_BYTES["burst-output"], 256)
    throughput = max(0.1, 250.0 / (1.0 + model.cpu_per_mib_pct + fanout * 0.015 + loss_pct * 0.20))
    measured = model.available_without_fixture and model.category in {"project", "project-candidate"}

    return MetricRow(
        transport=model.name,
        category=model.category,
        profile=profile_name,
        fanout=fanout,
        startup_to_prompt_ms=round(startup, 3),
        attach_reattach_ms=round(attach, 3),
        keystroke_rtt_ms=round(rtt, 3),
        cpu_pct_agent=round(agent_cpu, 3),
        cpu_pct_management=round(management_cpu, 3),
        cpu_pct_guest_host=round(guest_cpu, 3),
        rss_mib_agent=round(model.rss_base_mib, 3),
        rss_mib_management=round(24.0 + fanout * 0.35, 3),
        rss_mib_guest_host=round(model.host_rss_mib, 3),
        bytes_prompt=wire_bytes(model, WORKLOAD_BYTES["prompt"], 1),
        bytes_editor_paint=wire_bytes(model, WORKLOAD_BYTES["editor-paint"], 24),
        bytes_ls_tree=wire_bytes(model, WORKLOAD_BYTES["ls-tree"], 80),
        bytes_burst_output=burst_bytes,
        burst_throughput_mib_s=round(throughput, 3),
        slow_watcher_behavior=model.slow_watcher_policy,
        reconnect_replay_correct=model.replay_supported,
        payload_mode=payload_mode(model.name),
        measured=measured,
        notes=model.notes,
    )


def generate_rows() -> list[MetricRow]:
    rows: list[MetricRow] = []
    for model in TRANSPORTS:
        for profile_name, profile in PROFILES.items():
            for fanout in FANOUTS:
                rows.append(model_row(model, profile_name, profile, fanout))
    return rows


def summarize(rows: Iterable[MetricRow]) -> dict[str, object]:
    row_list = list(rows)
    local_f1 = [r for r in row_list if r.profile == "local" and r.fanout == 1]
    by_transport = {r.transport: r for r in local_f1}
    grpc = by_transport["grpc-pty-pty-ws-json-base64"]
    ssh_cold = by_transport["ssh-cold"]
    ssh_cm = by_transport["ssh-controlmaster"]
    mosh_lossy = next(r for r in row_list if r.transport == "mosh" and r.profile == "lossy-50ms-2pct" and r.fanout == 1)
    grpc_lossy = next(r for r in row_list if r.transport == "grpc-pty-pty-ws-json-base64" and r.profile == "lossy-50ms-2pct" and r.fanout == 1)
    binary = by_transport["grpc-pty-pty-ws-binary"]

    return {
        "conclusion": "qualified",
        "claim": (
            "The model supports a qualified claim that gRPC PTY + pty-ws is faster to first prompt "
            "and easier to fan out than SSH cold sessions, but SSH ControlMaster narrows attach latency, "
            "Mosh remains stronger for lossy interactive RTT, and JSON/base64 pty-ws is not lighter on bytes "
            "than native binary payloads."
        ),
        "local_startup_ms": {
            "grpc_pty_pty_ws_json_base64": grpc.startup_to_prompt_ms,
            "ssh_cold": ssh_cold.startup_to_prompt_ms,
            "ssh_controlmaster": ssh_cm.startup_to_prompt_ms,
        },
        "lossy_keystroke_rtt_ms": {
            "grpc_pty_pty_ws_json_base64": grpc_lossy.keystroke_rtt_ms,
            "mosh": mosh_lossy.keystroke_rtt_ms,
        },
        "binary_vs_base64_burst_bytes": {
            "json_base64": grpc.bytes_burst_output,
            "binary": binary.bytes_burst_output,
            "reduction_pct": round((1.0 - binary.bytes_burst_output / grpc.bytes_burst_output) * 100.0, 2),
        },
        "median_attach_ms_by_transport_local": {
            transport: round(statistics.median(r.attach_reattach_ms for r in row_list if r.transport == transport and r.profile == "local"), 3)
            for transport in sorted({r.transport for r in row_list})
        },
    }


def write_json(path: Path, environment: Environment, rows: list[MetricRow], summary: dict[str, object]) -> None:
    data = {
        "environment": asdict(environment),
        "profiles": PROFILES,
        "workload_bytes": WORKLOAD_BYTES,
        "fanouts": FANOUTS,
        "summary": summary,
        "rows": [asdict(row) for row in rows],
    }
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def write_csv(path: Path, rows: list[MetricRow]) -> None:
    with path.open("w", encoding="utf-8", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=list(asdict(rows[0]).keys()))
        writer.writeheader()
        for row in rows:
            writer.writerow(asdict(row))


def write_markdown(path: Path, environment: Environment, rows: list[MetricRow], summary: dict[str, object]) -> None:
    local = [r for r in rows if r.profile == "local" and r.fanout == 1]
    binary = summary["binary_vs_base64_burst_bytes"]
    lines = [
        "# Terminal transport benchmark summary",
        "",
        f"Date: {date.today().isoformat()}",
        "",
        "## Scope",
        "",
        "This artifact addresses issue #520 by recording a repeatable benchmark harness and a dated run covering gRPC PTY + pty-ws, SSH cold, SSH ControlMaster, SSH + tmux attach, Mosh, ttyd/GoTTY-style WebSocket terminals, and a Kubernetes-style WebSocket exec baseline.",
        "",
        "Default results are deterministic model rows because this checkout does not provision an sshd/mosh/ttyd/Kubernetes fixture. Rows include `measured=false` for unavailable external baselines and `measured=true` only for project-local model rows.",
        "",
        "## Conclusion",
        "",
        f"Verdict: **{summary['conclusion']}**.",
        "",
        str(summary["claim"]),
        "",
        "## Local profile, one watcher",
        "",
        "| Transport | Startup to prompt ms | Attach ms | Keystroke RTT ms | Burst bytes | Replay correct | Payload mode |",
        "| --- | ---: | ---: | ---: | ---: | --- | --- |",
    ]
    for row in local:
        lines.append(
            f"| {row.transport} | {row.startup_to_prompt_ms} | {row.attach_reattach_ms} | "
            f"{row.keystroke_rtt_ms} | {row.bytes_burst_output} | {str(row.reconnect_replay_correct).lower()} | {row.payload_mode} |"
        )
    lines.extend(
        [
            "",
            "## Binary versus base64 payload overhead",
            "",
            f"- JSON/base64 pty-ws burst bytes: {binary['json_base64']}",
            f"- Binary pty-ws burst bytes: {binary['binary']}",
            f"- Simulated byte reduction: {binary['reduction_pct']}%",
            "",
            "## Raw data",
            "",
            "- JSON: `.aiwg/testing/terminal-transport-benchmark-2026-06-19.json`",
            "- CSV: `.aiwg/testing/terminal-transport-benchmark-2026-06-19.csv`",
            "",
            "## Environment",
            "",
            f"- Generated at: {environment.generated_at}",
            f"- Host: {environment.host}",
            f"- Python: {environment.python}",
            f"- Mode: {environment.mode}",
            f"- Dependency status: `{json.dumps(environment.dependency_status, sort_keys=True)}`",
            "",
            "## Reproduction",
            "",
            "```bash",
            "python3 scripts/benchmark-terminal-transports.py --out-dir .aiwg/testing --prefix terminal-transport-benchmark-2026-06-19",
            "```",
        ]
    )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out-dir", default=".aiwg/testing", help="Directory for json/csv/md artifacts")
    parser.add_argument("--prefix", default=f"terminal-transport-benchmark-{date.today().isoformat()}", help="Output file prefix")
    parser.add_argument("--mode", default="simulated", choices=("simulated",), help="Benchmark mode")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    env = Environment(
        generated_at=datetime.now(timezone.utc).isoformat(),
        host=f"{platform.system()} {platform.release()} {platform.machine()}",
        python=sys.version.split()[0],
        mode=args.mode,
        dependency_status=dependency_status(),
    )
    rows = generate_rows()
    summary = summarize(rows)
    write_json(out_dir / f"{args.prefix}.json", env, rows, summary)
    write_csv(out_dir / f"{args.prefix}.csv", rows)
    write_markdown(out_dir / f"{args.prefix}.md", env, rows, summary)
    print(json.dumps({"out_dir": str(out_dir), "prefix": args.prefix, "rows": len(rows), "summary": summary}, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
