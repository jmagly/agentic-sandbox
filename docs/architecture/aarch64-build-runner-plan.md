# aarch64 Build Runner Plan (mutsu)

**Status:** Deferred — documented now, implement when ready.
**Target host:** `mutsu` (Mac Mini, Apple M4)
**Owner-decision pending:** runtime-on-mac vs. cross-compile-on-mac (see § 5)

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
| Disk | 245 GB total, **only 16 GB free** (94% full) |
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

## 3. The constraint that drives everything

**16 GB free disk.** A useful build runner needs ≥100 GB free (Rust target dirs are 5–15 GB per workspace, container caches push that further, VM images add 20–50 GB). Before mutsu can play any of the roles below, **someone has to reclaim disk** — either uninstall unrelated work, attach external storage, or rebuild it.

This is the first item on the implementation checklist regardless of which architectural path is chosen.

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
- Disk hungry: VM image (~30 GB) + Linux toolchains (~20 GB) + Rust target dirs (~15 GB) = ~65 GB just for the guest. Doesn't fit on 16 GB free disk; need ≥100 GB free first.
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

**Defer; prefer Option A when implemented.** Rationale:

1. The release-pipeline audit's Phase 2 acceptance criteria do not require aarch64. We can ship usable releases (x86_64 gnu + musl) without it. aarch64 is adoption-quality, not correctness-quality.
2. The 16 GB disk constraint blocks Options B and C immediately. Option A is the only path that fits today.
3. Option A unlocks **two** valuable targets — `aarch64-apple-darwin` (which we can't get any other way) and `aarch64-unknown-linux-gnu` (via cross-compile) — for the cost of one runner setup.
4. The runtime-on-macOS question (Option C) is its own project and should be scoped from product/architecture intent, not as a side effect of fixing CI.

**When ready, the work order is:**

1. **Reclaim disk on mutsu** (target: ≥100 GB free). Out of scope for this doc — operator decision.
2. **Install Homebrew, then via brew:** `act_runner`, `cosign`, `syft`, `gh`, optionally `colima`.
3. **Install Rust:** `rustup default stable`, then `rustup target add aarch64-unknown-linux-gnu`.
4. **Install `cargo-zigbuild`** (lighter than `cross` for cross-compile, no Docker required): `brew install zig && cargo install --locked cargo-zigbuild`.
5. **Register `act_runner` with Gitea**, scoped to the agentic-sandbox repo. Label the runner `aarch64-macos` so workflows can select it.
6. **Extend `release-binaries` matrix in `ci.yaml`** with two new entries:
   - `target: aarch64-unknown-linux-gnu`, `runs-on: [self-hosted, aarch64-macos]`, build via `cargo zigbuild --target aarch64-unknown-linux-gnu`
   - `target: aarch64-apple-darwin`, `runs-on: [self-hosted, aarch64-macos]`, build via `cargo build --target aarch64-apple-darwin`
7. **Test on a throwaway `vX.Y.Z-rc.1` tag** before promoting to a stable release. Verify the tarballs are well-formed and execute on a real aarch64 host.

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
