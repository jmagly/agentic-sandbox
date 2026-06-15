# Container Runtime

Container parity with VM agentic-dev landed in `2026.5.0` under the
#181 epic (issues #182–#186). The dashboard, REST surface, and AIWG
bridge treat containers as first-class workloads alongside QEMU/KVM
VMs — same lifecycle vocabulary, same loadout selector, same mission
dispatch flow.

This document is the reference for operators picking a runtime and for
integrators wiring container instances into the AIWG bridge. The Rust
source of truth is
[`management/src/docker_runtime.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/docker_runtime.rs);
the HTTP surface that wraps it is
[`management/src/http/containers.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/http/containers.rs).

---

## Public API

`docker_runtime` is the single chokepoint for Docker shell-outs. Every
container lifecycle operation funnels through these functions:

| Symbol | Purpose |
|---|---|
| `DockerMonitorConfig` | Poll cadence + orphan-age threshold; loaded from env (`DOCKER_MONITOR_ENABLED`, `DOCKER_POLL_INTERVAL_SECS`, `DOCKER_ORPHANED_AGE_SECS`). |
| `ContainerInfo` / `ContainerStatus` | Normalized `docker ps` row — `Running`, `Stopped`, or `Other(raw)`. `finished_at` populated for stopped containers. |
| `SpawnOpts` | `env: Vec<(String,String)>`, `labels: Vec<(key, value)>`, `mounts: Vec<(host, container)>`, `network: Option<String>`, `cmd: Vec<String>`. |
| `list_containers()` | `docker ps -a --filter label=agentic-sandbox=true`. Managed containers only — we never surface containers we did not spawn. |
| `spawn_container(name, image, opts)` | Runs `docker run -d --label agentic-sandbox=true --name {name} --add-host host.docker.internal:host-gateway …`. Returns the container ID. |
| `start_container(name)` / `stop_container(name, timeout)` | Idempotent lifecycle verbs over the same label-filtered set. |
| `remove_container(id)` | `docker rm -f` on a single ID. |
| `get_container_by_name(name)` | Convenience lookup over `list_containers()`. |
| `spawn_docker_monitor(config, metrics)` | Background task: polls every `poll_interval_secs`, emits `container.*` lifecycle events, sweeps orphans older than `orphaned_age_secs`. |

The `--add-host host.docker.internal:host-gateway` is unconditional on
Linux. Without it the in-container agent's default
`MANAGEMENT_SERVER=host.docker.internal:8120` does not resolve and the
container starts but immediately fails its first gRPC dial. Docker
no-ops the flag on Mac/Windows where the host gateway is native.

---

## Runtime selection: VM vs container

Both runtimes register against the same `OutputAggregator`, speak the
same gRPC contract from `agent-rs`, and surface in the same dashboard
sidebar. They differ where the substrate differs.

| Dimension | VM (QEMU/KVM) | Container (Docker) |
|---|---|---|
| **Isolation** | Full hardware virtualization. Kernel boundary between host and workload. | Process namespace. Shared kernel. |
| **Startup time** | 30–90 s cold (cloud-init runs once); 5–15 s warm. | 1–3 s typical for `agentic/agent:dev`-derived images. |
| **Resource overhead** | ~512 MB RAM floor per VM (kernel + systemd + journald). Dedicated virtual disk. | ~50 MB RAM floor. Layered filesystem; no per-instance kernel. |
| **Network** | Libvirt-managed bridge (`192.168.122.0/24` default). Per-VM IP. `agentshare` profile gets `--network none` for isolation. | Docker bridge or `--network host`. Reaches the host via `host.docker.internal:host-gateway`. |
| **Persistence** | Disk image survives `virsh destroy`; only `provision-vm.sh --destroy` wipes it. | Container filesystem is ephemeral unless mounts are bound. Use `mounts: [(host_path, /workdir)]` for persistence. |
| **AIWG framework install** | Baked into the cloud-init seed by `provision-vm.sh` via loadout. | Baked into the image at build time; `claude` / `codex` / `opencode` images rebase onto `agentic/agent:dev`. |
| **Operator escape hatch** | `virsh console`, `ssh agent@<ip>`. | `docker exec -it <name> bash`. |
| **Crash recovery** | `crash_loop.rs` detector triggers `provision-vm.sh` rebuild. See [`crash-loop.md`](crash-loop.md). | Monitor sweeps stopped containers older than `orphaned_age_secs` (default 1 h). No auto-rebuild — operator decides. |

### When to pick a VM

- The workload runs untrusted code, downloads arbitrary binaries, or
  needs to exercise kernel features the container runtime forbids
  (raw sockets, ptrace of arbitrary PIDs, loading kernel modules).
- The workload needs to survive container daemon restarts independent
  of host reboot.
- The mission persists for hours and the storage cost of a virtual
  disk is acceptable.
- The mission needs the `agentshare --network none` isolation tier
  (forensics / red-team profiles).

### When to pick a container

- The workload is a short-lived agent task (minutes to ~1 h).
- Fast iteration: rebuild image once, spawn dozens of fresh instances.
- The toolchain in `agentic/agent:dev` is sufficient (Python via uv,
  Node via fnm, Go, Rust via rustup, ripgrep/fd/bat/jq/delta/xh,
  cmake/ninja/meson, aider pinned to Python 3.12, `gh` + `gh copilot`).
- The provider image (claude / codex / opencode) is one of the rebased
  variants that already speak the agent protocol.

---

## Image catalog

Container images are layered: a shared dev toolchain at the bottom,
provider-specific images on top.

| Image | Purpose | Built from |
|---|---|---|
| `agentic/agent:dev` | Shared dev toolchain layer. Mirrors the `agentic-dev` VM profile's `apt`/`uv`/`fnm`/`rustup` package set. /etc/profile.d snippet stabilizes PATH across login shells. | Debian base + AIWG bootstrap. See `CHANGELOG.md` 2026.5.0 entry for #182. |
| `agentic/claude:latest` | Claude Code CLI on top of `agentic/agent:dev`. | Rebased onto shared base for parity (#183). |
| `agentic/codex:latest` | OpenAI Codex CLI on top of `agentic/agent:dev`. | Rebased onto shared base (#184). |
| `agentic/opencode:latest` | OpenCode CLI on top of `agentic/agent:dev`. | Rebased onto shared base (#185). |
| `agentic/automation-control:latest` | Blueprint for orchestrator-driven TUI control sessions. Includes Codex, Aider, shared dev tools, and `agentic-provider-inventory` without bundling credentials. | Extends `agentic/codex:latest` (#346). |

The CI smoke matrix (#186) builds each image and asserts:

- `python --version`, `node --version`, `go version`, `cargo --version`
  all resolve.
- `rg --version`, `fd --version`, `bat --version`, `jq --version`,
  `xh --version`, `grpcurl --version` all resolve.
- The agent binary inside the image dials the management server and
  registers within the smoke window.

## Automation-control blueprint

Use `agentic/automation-control:latest` when an external orchestrator needs a
general-purpose sandbox session it can observe, search, and drive through the
PTY control plane. The image intentionally does not embed secrets or auto-launch
provider login flows from global env. Start with the credential-free probe, then
use the inventory and readiness helpers before starting a managed provider TUI:

```bash
agentic-provider-inventory
agentic-provider-readiness codex
agentic-codex-automation
agentic-claude-automation
```

`agentic-codex-automation` prefers `OPENAI_API_KEY_FILE` or
`AGENTIC_CREDENTIAL_DIR/openai_api_key`, then sets `OPENAI_API_KEY` only in the
final provider process. `agentic-claude-automation` does the same for
`ANTHROPIC_API_KEY_FILE` or `AGENTIC_CREDENTIAL_DIR/anthropic_api_key`.
Both wrappers support `AGENTIC_PROVIDER_HOME` for isolated provider
home/config/cache directories.

`agentic-provider-readiness` emits structured tab-separated readiness rows:
provider, CLI presence/version, auth state, and error class. It does not print
credential values.

Then launch provider TUIs only after the orchestrator has satisfied its
credential and Controller-input policy gates. The target model for automated
provider launch is ADR-028: startup profiles reference credential ids, the
credential broker issues session-scoped leases, and provider launchers consume
leased files from a per-session credential directory. Container instances should
receive those leases through tmpfs/secret-style mounts scoped to the managed
container/session, not through image-baked credentials or `docker run -e`
provider tokens.

---

## REST surface

Container lifecycle lives at `/api/v1/containers/*`, mirroring the
shape of `/api/v1/vms/*`. The full list is in
[`management/src/http/containers.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/http/containers.rs);
the relevant endpoints are:

- `GET    /api/v1/containers` — list managed containers.
- `POST   /api/v1/containers` — create + spawn (auto-injects agent
  bootstrap env, generates 256-bit secret).
- `GET    /api/v1/containers/{name}` — single-container detail.
- `POST   /api/v1/containers/{name}/start` — start a stopped container.
- `POST   /api/v1/containers/{name}/stop` — graceful stop with timeout.
- `DELETE /api/v1/containers/{name}` — `docker rm -f`.

The image catalog endpoint
(`GET /api/v1/container-images` →
[`management/src/http/container_images.rs:56`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/http/container_images.rs))
returns the curated provider image set the dashboard offers in the
Create dialog.

PTY exec inside a container is **not** part of this surface — that
lives behind the `pty-ws/v1` binding (see [`pty-rendering.md`](pty-rendering.md))
which attaches to whatever the container entrypoint produces via the
existing in-container agent path.

---

## AIWG bridge integration

When `AIWG_SERVE_ENDPOINT` is set, the management server registers
itself as an A2A executor (see [`aiwg-executor.md`](aiwg-executor.md)).
Mission dispatch lands at
`POST /api/v1/sessions/:id/dispatch` and routes to either a VM or a
container depending on the session's recorded runtime.

- The bridge does **not** know or care which runtime backs an
  instance. It addresses by `(instance_id, session_id)`.
- The dispatch handler resolves the instance to its runtime, then
  invokes the matching session-attach path.
- Container instances participate in the same `mission.*` event
  vocabulary the executor contract emits — `mission.dispatched`,
  `mission.completed`, `mission.failed`. The events stream over the
  same `/ws/executors/{id}` channel.

The 2026.5.0 `server_hello` capability banner (#190) advertises both
runtimes; AIWG's `replayCapable` gate flips on for either.

---

## Loadout interaction

Today's published loadout profiles (`images/qemu/profiles/`) are
VM-only — `agentic-dev.yaml`, `agentic-dev-cloud-init.yaml`. The
loadout schema described in [`LOADOUTS.md`](LOADOUTS.md) is reused
for container instances by setting an explicit runtime on the
manifest. Container "loadouts" today are effectively the choice of
image (for example `agentic/claude:latest`, `agentic/codex:latest`, `agentic/opencode:latest`, or `agentic/automation-control:latest`) plus mounts and env;
the formal `runtime:` field unifies that with the VM profile syntax.

When provisioning a container from the dashboard:

1. Pick **Container** in the Runtime dropdown of the Unified Instances
   Create dialog (#178).
2. Pick an image from the curated `agentic/*:latest` provider list.
3. Optionally add bind mounts (host path → `/workdir`-style container
   path) for persistence. v2 admin Docker provision accepts `mounts`
   as `host_path:container_path` strings.
4. The dashboard issues `POST /api/v1/containers` with auto-injected
   non-secret bootstrap env plus secure transport material.

For v2 admin Docker provision, `agentshare: true` creates a per-instance host
workspace under `AGENTIC_SANDBOX_DOCKER_WORKSPACE_ROOT` or
`/var/lib/agentic-sandbox/workspaces` and bind-mounts it at `/workspace`.
If a caller supplies an explicit `/workspace` mount, that mount wins. Docker
AgentCards advertise `adapter-command/v1` only when a `/workspace` mount is
available, so orchestrators can treat the extension as a live capability
contract rather than an unconditional server feature.

---

## Operational notes

- **Orphan cleanup is opt-in.** `DOCKER_MONITOR_ENABLED=false` disables
  the background sweep entirely. Default prefix filter is `task-` so
  operator-spawned `agent-*` containers are never auto-deleted
  (regression-proofed in 2026.5.0 after a5c897f / 005e471 / 24e1cf9 /
  2e76a0d / 9dd7711).
- **Stop ≠ delete.** The dashboard's Stop button calls
  `POST /api/v1/containers/{name}/stop` and leaves the container in
  Stopped state so the operator can restart or inspect it. Force-off
  goes through the same path with timeout 0.
- **Container metrics** flow into the same `Metrics` aggregator as VM
  metrics (`management/src/telemetry/metrics.rs`). See
  [`telemetry.md`](telemetry.md) for the label scheme.
- **Lifecycle events** emit through the same SSE stream as VM events
  (`/api/v1/events?follow=true`). See [`transport-audit.md`](transport-audit.md).

---

## See also

- [`LOADOUTS.md`](LOADOUTS.md) — loadout manifest schema (VM today,
  container `runtime:` field per #178).
- [`aiwg-executor.md`](aiwg-executor.md) — full AIWG bridge contract.
- [`pty-rendering.md`](pty-rendering.md) — PTY attach over the
  `pty-ws/v1` binding (works against containers via in-container agent).
- [`crash-loop.md`](crash-loop.md) — VM-specific auto-remediation
  (container parity is operator-driven for now).
- [`telemetry.md`](telemetry.md), [`transport-audit.md`](transport-audit.md)
  — observability for either runtime.
- `CHANGELOG.md` — 2026.5.0 entry for the #181 epic.
