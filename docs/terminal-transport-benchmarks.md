# Terminal transport benchmarks

This document describes the repeatable benchmark harness for terminal
transport issue #520.

## Purpose

The harness qualifies whether the project can claim that the gRPC PTY path is
faster and lighter than SSH-based alternatives. It covers these candidates:

- gRPC PTY through `AgentPtyBridge` plus `pty-ws` attach, modeled as the current
  JSON/base64 client-facing path.
- A candidate binary `pty-ws` payload mode.
- SSH cold connection.
- SSH with ControlMaster/ControlPersist.
- SSH plus tmux attach.
- Mosh.
- ttyd/GoTTY-style WebSocket terminal.
- Kubernetes-style WebSocket exec/attach.

## Procedure

Run the deterministic harness from the repository root:

```bash
python3 scripts/benchmark-terminal-transports.py --out-dir .aiwg/testing --prefix terminal-transport-benchmark-2026-06-19
```

The harness writes JSON, CSV, and Markdown artifacts. The default mode is
simulated because a normal checkout does not include sshd, mosh-server, ttyd, or
a Kubernetes API fixture. Each row carries a `measured` flag and a dependency
status map so launch claims do not overstate the evidence.

## Metrics

The generated rows include:

- cold startup to first prompt;
- attach and reattach latency;
- keystroke round-trip latency for local, 50 ms, 150 ms, and lossy profiles;
- CPU and RSS estimates for agent, management, and guest terminal host;
- bytes on wire for prompt, editor repaint, tree listing, and burst output
  workloads;
- burst-output throughput;
- slow watcher behavior;
- fanout cost for 1, 4, 16, and 32 watchers;
- reconnect and replay correctness;
- binary versus base64 `pty-ws` payload overhead.

## Verification

Use these checks before relying on a benchmark artifact:

```bash
python3 -m py_compile scripts/benchmark-terminal-transports.py
python3 scripts/benchmark-terminal-transports.py --out-dir .aiwg/testing --prefix terminal-transport-benchmark-2026-06-19
```

For launch-facing language, link to the dated Markdown summary and state the
verdict as qualified unless a future fixture-backed run replaces the simulated
baseline rows with measured rows.
