# Platform Support

This page is the canonical reference for what agentic-sandbox runs on today, what's on the roadmap, and how the runtime abstraction decouples agent instances from their hosting substrate. Use it before opening compatibility issues or proposing a new backend.

## Compatibility Matrix

| OS / Image                  | libvirt+QEMU        | Proxmox            | Docker             | containerd | Apple `container` | Status        |
|-----------------------------|---------------------|--------------------|--------------------|------------|-------------------|---------------|
| Ubuntu agentic-dev          | ✓ shipping          | planned (#119)     | ✓ shipping         | planned    | spike (#488)      | stable        |
| Alpine agentic-dev          | planned (#118)      | planned (#119)     | planned (#118)     | —          | —                 | wave 6        |
| (others)                    | —                   | —                  | —                  | —          | —                 | not planned   |

`✓ shipping` means the path is exercised in CI / by deploy scripts on `main` today. `planned (#N)` tracks the issue that will land the work. `spike (#N)` means the provider is under feasibility validation and is not yet supported. `—` means there are no plans; users may make it work locally but it is not supported.

## Supported VM Images

### Ubuntu agentic-dev (current)

Defined by [`images/qemu/profiles/agentic-dev.yaml`](../images/qemu/profiles/agentic-dev.yaml). Ships with Node.js 22 LTS, Python 3 + venv, build-essential, common dev tools (ripgrep, fd-find, jq, htop, tmux, vim), and the `aiwg` global npm package. Cloud-init stages secure transport material and starts `agent-client.service` on boot; legacy TCP agent secret injection is retired. Unmanaged direct-runtime SSH keys are omitted by default for this managed profile; use gateway-mediated SSH or set `AGENTIC_ENABLE_DIRECT_RUNTIME_SSH=1` only for explicit dev/break-glass access.

Provision with:

```bash
./images/qemu/provision-vm.sh agent-01 --profile agentic-dev --agentshare --start
```

Loadouts layer additional installs on top of the base profile — see [LOADOUTS.md](LOADOUTS.md).

### Alpine agentic-dev (target — #118)

Tracked by issue #118. Goal: a musl-based image whose root filesystem fits in roughly a quarter of the Ubuntu footprint, so we can boot more concurrent agents on the same host. Requires the musl agent-client target below.

## Hypervisors

### libvirt + QEMU (current)

The supported control plane for VMs is libvirt over `qemu:///system`. The management server orchestrates VM lifecycle (`Define`, `Create`, `Destroy`, `Shutdown`) and watches `virConnectDomainEventCallback` for state changes. Networking uses an isolated libvirt NAT network; storage uses qcow2 overlays on a base image to keep per-VM disk under a few hundred megabytes.

### Proxmox (target — #119, #120)

Tracked by issues #119 (runtime abstraction) and #120 (Proxmox backend). Phase 1 of the runtime work introduces a `Runtime` trait whose libvirt implementation is the current code path; the Proxmox implementation in #120 will be a parallel implementor that delegates to the Proxmox API. No control-plane changes for the dashboard or CLI; the change is transparent.

## agent-rs Build Targets

### glibc (current)

The default build target is `x86_64-unknown-linux-gnu`, built with `cargo build --release` against the Debian 12 (`bookworm`) toolchain. Produces a ~6 MB stripped binary linked against glibc 2.36. Suitable for the Ubuntu agentic-dev image and any modern Debian/Ubuntu derivative.

```bash
cd agent-rs
cargo build --release
```

### musl (target — #115)

Tracked by issue #115. The musl target (`x86_64-unknown-linux-musl`) produces a fully static binary that runs unchanged on Alpine, Distroless, and busybox-based images. Build invocation will be:

```bash
cargo build --release --target x86_64-unknown-linux-musl
```

Configuration will land in `agent-rs/.cargo/config.toml`. The musl binary is a prerequisite for the Alpine agentic-dev image (#118).

## Container Runtimes

### Docker (current)

The container runtime path is implemented in [`management/src/docker_runtime.rs`](../management/src/docker_runtime.rs) and surfaced via the `/api/v1/containers` REST endpoints and `sandboxctl container *` CLI verbs (see [cli-design.md](cli-design.md)). Containers run as managed instances alongside VMs and carry the `agentic-sandbox=true` label. They are first-class citizens on the dashboard.

The reference Dockerfiles are in [`deploy/docker/`](../deploy/docker/) — `Dockerfile.agent-rust` and `Dockerfile.management` are exercised by `docker-compose.production.yaml`.

### containerd (planned)

No issue yet. The runtime abstraction in #119 is intended to make a future containerd backend a parallel implementor of `Runtime` rather than a fork of the Docker path.

### Apple `container` (spike — #438, #488, #489)

Apple Silicon macOS support is being evaluated through Apple's open source `container` project. That runtime runs OCI Linux containers as lightweight per-container virtual machines on macOS, which may map to agentic-sandbox's isolation model without a Parallels-specific backend.

This path is not supported yet. Issue #488 must first prove the runtime contract on an Apple Silicon macOS 26 host: image pull/run, management connectivity, workspace or agentshare setup, bootstrap enrollment, secure transport, logs/session observation, and cleanup. If the spike recommends proceeding, #489 implements an explicit provider such as `runtime.provider = "apple-container"` behind the management runtime abstraction. Provider selection must be explicit; do not treat generic macOS detection as support.

## Runtime Abstraction (#119)

The `runtime/v1` A2A extension ([`docs/contracts/extensions/runtime/v1/spec.md`](contracts/extensions/runtime/v1/spec.md)) carries the executing substrate metadata on every Task. AgentCards declare a `runtime` capability and optional `loadout` parameter, so consumers can route a task to a specific instance type (`vm-qemu`, `container-docker`, etc.) without knowing the management server's internal topology. ADR-022 (three-surface architecture) splits this metadata across the admin surface (for orchestration decisions) and the A2A per-instance surface (for client task routing).

The current implementation hard-codes libvirt+QEMU in the management server. The roadmap (#119, #120) refactors this into a trait so adding a new backend is implementing one Rust trait and registering it.

## Roadmap

Tracked work that affects this matrix:

- **#115** — musl static build for agent-rs
- **#118** — Alpine agentic-dev image (depends on #115)
- **#119** — Runtime abstraction trait
- **#120** — Proxmox backend (depends on #119)
- **#198** — `runtime/v1` extension parameters (`runtime`, `loadout` metadata on Task)
- **#438** — macOS host support via Apple `container` provider
- **#488** — Apple `container` feasibility spike
- **#489** — Apple `container` provider implementation after spike

See [ECOSYSTEM.md](ECOSYSTEM.md) for the wider Phase 1-6 plan.

## Not Supported

Anything not in the matrix is not supported. In particular:

- Windows hosts as a hypervisor (KVM is Linux-only; we have no plan to add Hyper-V)
- Intel Mac hosts
- macOS hosts before the Apple `container` support target identified in #438/#488
- Generic macOS hypervisor support through HVF, Parallels, Tart, Lima, or vfkit unless a dedicated provider issue accepts that backend
- 32-bit architectures
- BSD hosts

Container runtime support runs through whatever Docker / containerd compatibility their respective projects provide. Apple `container` support is provider-specific and remains unavailable until #488 validates the contract and #489 lands the explicit provider.

## Cross-References

- [welcome.md](welcome.md) — entry point, quick links
- [ECOSYSTEM.md](ECOSYSTEM.md) — overall roadmap
- [LOADOUTS.md](LOADOUTS.md) — profile + loadout system layered on top of images
- [contracts/extensions/runtime/v1/spec.md](contracts/extensions/runtime/v1/spec.md) — `runtime/v1` A2A extension
- [runtime-parity.md](runtime-parity.md) — VM ↔ container parity discussion
