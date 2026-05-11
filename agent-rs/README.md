# agent-rs — Agentic Sandbox Agent Client (Rust)

The agent client that runs inside every agent VM. Connects to the management server over gRPC, executes commands and PTY sessions, and streams output, logs, and health back over a bidirectional stream.

See [`docs/welcome.md`](../docs/welcome.md) and [`docs/grpc-architecture.md`](../docs/grpc-architecture.md) for project-level context.

## What's in this crate

| Module                       | Responsibility                                                                                                                 |
|------------------------------|--------------------------------------------------------------------------------------------------------------------------------|
| `src/main.rs`                | Entry point. Parses `--server`, `--agent-id`, `--secret`, builds the gRPC channel, opens the bidirectional `Connect` stream, dispatches inbound commands. |
| `src/lib.rs`                 | Public types reused by tests: `StdinData`, `PtyControlMsg`, `RunningCommand`. Channel-typed senders for stdin and PTY control. |
| `src/claude.rs`              | Claude Code subprocess management: launches `claude` CLI, manages its lifecycle, parses structured output for task progress.   |
| `src/health.rs`              | Health probe surface: liveness/readiness checks, sub-system rollup, exposed via gRPC heartbeat plus optional HTTP `/healthz`.  |
| `src/metrics.rs`             | Sysinfo-backed metric collection: CPU, memory, disk, load average, uptime. Snapshot delivered to management via `Metrics` proto frame. |
| `src/metrics_exporter.rs`    | Optional Prometheus text-format exporter. Bind address configurable; gated on a feature/env flag.                              |

The wire types come from [`../proto/agent.proto`](../proto/README.md) via `tonic-build` (see `build.rs`).

## Build

### Default (glibc, dynamic)

```bash
cd agent-rs
cargo build --release
```

Produces `target/release/agent-client`. The `release` profile is tuned for size (`opt-level = "z"`, LTO, single codegen unit, strip).

The `default` feature is `systemd`, which enables `sd-notify` for proper `Type=notify` integration with the systemd unit (`READY=1`, `WATCHDOG=1`).

### Static musl (planned — #115)

Once #115 lands, a fully static binary will be available via:

```bash
cargo build --release --target x86_64-unknown-linux-musl
```

This is a prerequisite for the Alpine agentic-dev image (#118). See [`docs/platform-support.md`](../docs/platform-support.md) for the matrix.

## Configuration

The agent reads `--server`, `--agent-id`, `--secret` from CLI args or environment. The production path is to populate `/etc/agentic-sandbox/agent.env` (root-owned, mode 0600) and let the systemd unit `EnvironmentFile=` it in. See [`../deploy/agent.env.template`](../deploy/README.md).

Required variables:

```
AGENT_ID=agent-01
AGENT_SECRET=<64-hex-chars>
MANAGEMENT_SERVER=192.168.122.1:8120
HEARTBEAT_INTERVAL=30
AGENT_PROFILE=agentic-dev
```

The plaintext secret lives only inside the VM. The host stores the SHA-256 hash in `~/.config/agentic-sandbox/agent-tokens`.

## systemd Integration

The reference unit is [`../deploy/systemd/agent-client.service`](../deploy/README.md). Highlights:

- `Type=simple` with `Restart=always`, `RestartSec=5`
- `EnvironmentFile=-/etc/agentic-sandbox/agent.env`
- Hardened: `NoNewPrivileges=yes`, `ProtectSystem=strict`, `ProtectHome=read-only`, `PrivateTmp=yes`
- `ReadWritePaths=/mnt/inbox` so the agent can drop outputs into agentshare
- `MemoryMax=512M`, `CPUQuota=200%` resource limits

## Deploy

After modifying `agent-rs` code:

```bash
# Deploy to one VM
../scripts/deploy-agent.sh agent-01 --debug

# Rebuild + deploy to every running VM
../scripts/dev-deploy-all.sh --debug
```

These scripts read the plaintext secret from the VM (via `sudo cat`), not from the host's hash file — see the deployment workflow note in [`../CLAUDE.md`](../CLAUDE.md).

## Protocol Surface

Client side of `AgentService` in [`../proto/agent.proto`](../proto/README.md). Outbound (Agent → Management): registration, heartbeats, stdout/stderr/log chunks, command results, metrics snapshots, session reports. Inbound (Management → Agent): commands, PTY control (resize, signal, stdin), config updates, shutdown signals, pings.

The gRPC channel is plaintext on the libvirt NAT network. Authentication is the per-agent SHA-256-pinned bearer secret carried as a metadata header on every request.

## Testing

```bash
cargo test
```

The `lib.rs` types are public specifically so tests under `tests/` can drive the channels without spawning a real gRPC server.
