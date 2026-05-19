# aarch64 Build Runner Plan (mutsu)

**Status:** Live as of 2026-05-19 — both `aarch64-apple-darwin` and `aarch64-unknown-linux-gnu` matrix entries in `ci.yaml`. Build invocation switched from native `runs-on: mutsu` (act_runner) to **SSH-from-Linux-runner pattern** (per `fortemi/publish-sidecar.yml`) on 2026-05-19 because the native act_runner path has a documented reverse-proxy / gRPC task-fetch failure mode in this Gitea install.
**Target host:** `mutsu` (Mac Mini, Apple M4)
**Build host:** the self-hosted Linux runner SSHes to `manitcor@10.0.42.41` per build, clones the repo into `/Volumes/build/agentic-sandbox/builds/run-<RUN_ID>-<TARGET>/`, runs `cargo {build,zigbuild}`, scp's binaries back.
**Runner state:** `/Volumes/build/agentic-sandbox/{cargo,rustup,target,bin,builds}/` — toolchain, target cache, build scratch.
**Required secret:** `MUTSU_SSH_KEY` — PEM private key for `manitcor@10.0.42.41`.

**Removed 2026-05-19** (the daemon was claiming `runs-on: self-hosted` jobs intended for the Linux runner; failed CI run #393):
- Gitea runner id 15 (`mutsu`) — unregistered via API.
- LaunchAgent `~/Library/LaunchAgents/net.integrolabs.actrunner.plist` — unloaded + plist deleted.
- `~/Library/Application Support/agentic-sandbox-runner/` — removed.
- `/Volumes/build/agentic-sandbox/{runner,logs,builds}/` and `bin/{act_runner,run-act-runner.sh}` — removed.

**Kept on `/Volumes/build/agentic-sandbox/`** (still needed by the SSH-pattern builds):
- `cargo/` (CARGO_HOME, includes `cargo-zigbuild`)
- `rustup/` (stable toolchain + aarch64 std targets)
- `target/` (build cache)
- `bin/{zig,protoc}` (cross-link tools — symlinks into `zig/` and `protoc/`)
- `zig/`, `protoc/` (extracted tool roots)
- `repo/` (a clone, optional convenience)
- `cargo/config.toml` (`net.git-fetch-with-cli = true` — required for `git.integrolabs.net` deps)

**Owner-decision pending:** runtime-on-mac vs. cross-compile-on-mac (see § 5, Option C)

## 1. Why this exists

`docs/architecture/release-pipeline-audit.md` § Phase 2 calls out aarch64 as the deferred binary target. The current `release-binaries` matrix builds `x86_64-unknown-linux-gnu` and `x86_64-unknown-linux-musl` natively on the self-hosted x86_64 Linux runner. To produce `aarch64-unknown-linux-gnu` (or `aarch64-apple-darwin`) artifacts we need:

- A native aarch64 runner, **OR**
- A cross-compile setup (`cross`, requires Docker-in-Docker on the existing runner)

`mutsu` is an available Apple Silicon machine, so it is the natural candidate for a native aarch64 runner. This doc inventories its current state, lays out the three implementation paths, and explains the deferral.

## 2. Current state of mutsu (inventory: 2026-05-19)

| Property | Value |
|---|---|
| Host | `mutsu` (`10.0.42.41`) |
| OS | macOS 26.4.1 (build 25E253) |
| Kernel | Darwin 25.4.0 |
| CPU | Apple M4 (aarch64) |
| RAM | 16 GB |
| Internal disk | 245 GB total, **only 16 GB free** (94% full) — NOT usable for build state |
| **External disk** | **2 TB APFS at `/Volumes/build`, 1.6 TB free** (Thunderbolt, local, `nodev,nosuid`) — the build runner should live here |
| User available for automation | `manitcor` (via `mutsu-agent` SSH key) |
| Virtualization frameworks present | `Hypervisor.framework`, `Virtualization.framework`, `ParavirtualizedGraphics.framework` |

### Toolchains installed

| Tool | Status |
|---|---|
| Apple clang | ✓ (21.0.0) |
| git | ✓ (2.50.1 Apple Git-155) |
| Python | ✓ (3.9.6, system) |
| rustup | ✓ installed, **but no default toolchain set** (rustc/cargo not currently usable) |
| Xcode CLI tools | partial (update pending: 26.5) |

### Toolchains NOT installed

- Homebrew (no `brew` on PATH)
- Docker / Podman / Colima / OrbStack
- Tart / Lima / vfkit / qemu-system-aarch64 (no Linux-VM stack)
- Gitea runner (`act_runner`)
- cosign, syft, gpg, gh, go, node

### Network reachability

- Resolves `git.integrolabs.net` → `10.0.42.95` (Gitea internal). Same LAN, can register as a runner.

## 3. The constraint that drove everything — now resolved

The internal disk has 16 GB free (94% full), which would block any meaningful runner work. **The 2 TB external APFS drive at `/Volumes/build` (1.6 TB available) resolves this entirely.** All runner state — `act_runner` working dirs, Rust target trees, container caches, VM images — should live under `/Volumes/build/agentic-sandbox-runner/` (or a similar dedicated directory) so the internal disk pressure is irrelevant.

Existing tenants on `/Volumes/build` (for reference; runner work should not collide):

| Path | Size | Purpose |
|---|---|---|
| `/Volumes/build/fortemi/` | 130 GB | unrelated project |
| `/Volumes/build/bt6/` | 54 GB | unrelated project |
| `/Volumes/build/ollama/` | 33 GB | local LLM models |
| `/Volumes/build/hotm/` | 2.2 GB | unrelated project |

Proposed runner directory: `/Volumes/build/agentic-sandbox/` (sibling to the existing project dirs). Allocate ~150 GB for full Rust toolchains + target + container cache + a Linux VM image if Option B is later chosen.

This shifts the architectural recommendation: Options A **and** B are now both feasible. Option C still gated on its own product/architecture call.

## 4. Three architectural paths

### Option A — Native aarch64-macOS runner, cross-build to aarch64-linux

**What it is:** Install Gitea `act_runner` natively on macOS. Use rustup to build for both `aarch64-apple-darwin` (native) and `aarch64-unknown-linux-gnu` (via `cross` or `cargo-zigbuild`).

**Pros:**
- Simplest. No VM layer. macOS-native build for Mac users falls out for free.
- Apple clang already present.
- Lightest disk footprint.

**Cons:**
- `cross` requires Docker — would need Colima or OrbStack on mutsu (each adds 1–2 GB binary + 10+ GB for the image cache).
- `cargo-zigbuild` is lighter but newer; less battle-tested for the project's specific deps (a2a-rs, libvirt-rs).
- aarch64-linux artifacts are produced by a Linux-not-running-here process — harder to debug if something specific to that target breaks.

**Implementation work:** ~1 day (install Homebrew → cargo-zigbuild → act_runner → register → matrix entry in ci.yaml).

### Option B — Linux VM on mutsu, run a Linux runner inside it

**What it is:** Use Apple's `Virtualization.framework` (via Tart, Lima, or vfkit) to run an aarch64-Linux guest. Install `act_runner` inside that guest. The guest looks identical to a real aarch64 Linux runner.

**Pros:**
- Identical environment to the existing x86_64 Linux runner. Same `apt` packages, same `cross` config, same `make build`.
- Build artifacts are Linux-built — no cross-compile surprises.
- The guest can also exercise the runtime's container-runtime tests (Docker runs natively inside a Linux guest on aarch64).

**Cons:**
- Adds a VM layer (Tart / Lima maintenance).
- Disk hungry: VM image (~30 GB) + Linux toolchains (~20 GB) + Rust target dirs (~15 GB) = ~65 GB just for the guest. **Fits comfortably on `/Volumes/build` (1.6 TB free).**
- The runtime itself (`agentic-sandbox`) uses **KVM** which is Linux-only. A Linux-on-macOS guest doesn't have nested virtualization on Apple Silicon today — so this guest can build the runtime, but **cannot run the KVM-backed integration tests**. E2E in this guest would have to skip VM-runtime tests or fall back to container-runtime tests only.

**Implementation work:** ~2–3 days (Tart/Lima setup + Ubuntu image + Rust + Docker + act_runner inside).

### Option C — Run the runtime itself on macOS (port to Apple Virtualization)

**What it is:** A larger architectural shift. Make the `agentic-sandbox` runtime support macOS as a host, using `Virtualization.framework` (or wrapping Tart) instead of libvirt/QEMU/KVM.

**Pros:**
- Unlocks Apple Silicon as a first-class host platform.
- mutsu (and any Mac) becomes both a build runner AND a runtime host.

**Cons:**
- This is a **multi-week project**, not a CI fix. Touches `images/qemu/provision-vm.sh` (assumes libvirt), the management server's VM provisioning code path, the virtiofs shared-storage model (different on macOS), networking (libvirt vs bridge on macOS), cloud-init equivalent.
- Apple Silicon Linux VMs lack nested-virt — can't run nested KVM, may break some agent-loadout assumptions.
- macOS-host security model differs (no `seccomp`, no `cgroups`, different namespace model).
- Out of scope for the release-pipeline audit; tracked separately if pursued.

**Implementation work:** ~2–4 weeks of feature work, plus testing matrix expansion. Not a CI task.

## 5. Recommendation

**Prefer Option A; deferred only on bandwidth, not on viability.** Rationale:

1. The release-pipeline audit's Phase 2 acceptance criteria do not require aarch64. We can ship usable releases (x86_64 gnu + musl) without it. aarch64 is adoption-quality, not correctness-quality.
2. With `/Volumes/build` available, Option A is straightforward and Option B is also feasible. Option A unlocks **two** valuable targets — `aarch64-apple-darwin` (only path) and `aarch64-unknown-linux-gnu` (via cross-compile) — for the cost of one runner setup, no VM layer.
3. The runtime-on-macOS question (Option C) is its own project and should be scoped from product/architecture intent, not as a side effect of fixing CI.

**When ready, the work order (Option A) is:**

1. **All runner state lives on `/Volumes/build/agentic-sandbox/`** — never on internal disk. Specifically:
   - `RUNNER_HOME=/Volumes/build/agentic-sandbox/runner`
   - `CARGO_HOME=/Volumes/build/agentic-sandbox/cargo`
   - `CARGO_TARGET_DIR=/Volumes/build/agentic-sandbox/target`
   - `RUSTUP_HOME=/Volumes/build/agentic-sandbox/rustup`
   These get set in the `act_runner` service environment (and inherited by every job step).
2. **Install Homebrew** (one-line installer from brew.sh). Set `HOMEBREW_PREFIX` accordingly; Homebrew on Apple Silicon defaults to `/opt/homebrew` which is fine on the internal disk (small footprint, ~1 GB).
3. **Install via brew:** `act_runner`, `cosign`, `syft`, `gh`, `gnupg`, `zig`.
4. **Install Rust into the external dir:** `RUSTUP_HOME=/Volumes/build/agentic-sandbox/rustup CARGO_HOME=/Volumes/build/agentic-sandbox/cargo curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain stable -y`, then `rustup target add aarch64-unknown-linux-gnu`.
5. **Install `cargo-zigbuild`** (lighter than `cross`, no Docker required): `cargo install --locked cargo-zigbuild`.
6. **Register `act_runner` with Gitea**, scoped to the `agentic-sandbox` repo:
   ```bash
   cd /Volumes/build/agentic-sandbox/runner
   act_runner register --instance https://git.integrolabs.net --token <repo-runner-token> \
     --labels self-hosted,aarch64-macos,aarch64-darwin --name mutsu
   ```
   Install as a `launchd` service so it survives reboots.
7. **Extend `release-binaries` matrix in `ci.yaml`** with two new entries:
   - `target: aarch64-unknown-linux-gnu`, `runs-on: [self-hosted, aarch64-macos]`, build via `cargo zigbuild --target aarch64-unknown-linux-gnu`
   - `target: aarch64-apple-darwin`, `runs-on: [self-hosted, aarch64-macos]`, build via `cargo build --target aarch64-apple-darwin`
8. **Test on a throwaway `vX.Y.Z-rc.1` tag** before promoting to a stable release. Verify the tarballs are well-formed and execute on a real aarch64 host.

**Estimate:** ~half a day end-to-end now that disk isn't the blocker. Most of the time is the initial Rust toolchain install over the external bus.

## 6. Architecture implications (for record)

If Option C is later pursued, these are the architectural deltas to plan for:

| Subsystem | Linux/KVM (today) | macOS/Virtualization.framework |
|---|---|---|
| VM provisioning | `libvirt` + `virsh` + cloud-init | `Virtualization.framework` API, no cloud-init equivalent (would need first-boot script in image) |
| Disk format | qcow2 | raw + (optionally) APFS sparse |
| Networking | libvirt bridge, NAT, isolated | macOS bridge (`vmnet.framework`), different DHCP semantics |
| Shared storage | virtiofs | virtiofs IS supported via Virtualization.framework (post macOS 13) — same protocol, different host plumbing |
| Resource limits | cgroups | macOS does not have cgroups; rely on per-VM CPU/memory caps in Virtualization.framework, no per-PID granularity |
| Secrets / sandbox isolation | seccomp + namespaces | App Sandbox + Hypervisor.framework isolation — not 1:1 mapping |
| Console PTY | virtconsole + PTY | macOS pty allocation works, console binding via Virtualization API differs |
| The agent client (Rust binary inside guest) | Linux ELF binary | Same Linux ELF works inside a Linux guest; if running natively in macOS guest, need a Darwin build |

These deltas are NOT a checklist — they are an alert that this is real architectural work, not "swap a backend." Capture in an ADR before any code lands.

## 7. References

- `docs/architecture/release-pipeline-audit.md` § Phase 2 — what we said we'd defer
- `.gitea/workflows/ci.yaml` `release-binaries` job matrix — where the new entries would land
- Apple `Virtualization.framework` docs: <https://developer.apple.com/documentation/virtualization>
- Tart (Apple Silicon-friendly OCI-style VM tool): <https://tart.run>
- `cargo-zigbuild` (zig-based cross-compile, no Docker needed): <https://github.com/rust-cross/cargo-zigbuild>
- Gitea `act_runner` install: <https://docs.gitea.com/usage/actions/act-runner>
