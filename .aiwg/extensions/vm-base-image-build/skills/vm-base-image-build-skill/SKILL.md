---
name: vm-base-image-build-skill
description: Build or refresh the agentic-sandbox QEMU base image so it ships the current agent-client binary, the self-enrolling agent-client.service, the virtio-vsock guest transport, and an up-to-date kernel/libs/tool set. Use when the base image is stale, after an agent-client change, or when wiring the vsock transport (ADR-023/026, #561).
keywords:
  - base image
  - qemu image
  - build-base-image
  - agent-client
  - vsock
  - rebuild vm image
  - refresh base image
triggers:
  - "rebuild the qemu base image"
  - "refresh the vm base image"
  - "bake the agent into the base image"
  - "the base image is stale"
---

# VM Base Image Build / Refresh Flow

Procedure to (re)build the agentic-sandbox QEMU base image
(`/mnt/ops/base-images/ubuntu-server-<ver>-agent.qcow2`) so a provisioned VM
boots with a working, self-enrolling agent over the vsock transport — no
per-provision agent deploy required.

This codifies the fix for #561 (the prior image was 5 months stale: no
`agent-client`, a defunct `agentic-agent.service`, no vsock module). Anchors:
ADR-023 (transport-per-runtime → VM uses vsock), ADR-026 (host-mediated
enrollment), and `.aiwg/planning/vm-vsock-transport-implementation.md`.

## When to use

- The qemu base image is stale or missing the current agent.
- After an `agent-rs` change that must ship in the image by default.
- When wiring/validating the vsock transport for VMs.
- As a scheduled hygiene refresh (kernel/libs/tools currency).

## Preconditions

- Host has `qemu-img virt-install genisoimage virt-customize` (libguestfs-tools).
- An Ubuntu live-server ISO present under `ISO_DIR` (default `/mnt/ops/isos/linux`),
  pinned in `images/qemu/iso-pins.json` (integrity-verified at build).
- The current agent binary is built: `agent-rs/target/release/agent-client`
  (the build bakes it in; absent ⇒ it warns and builds without the agent).

## Steps

1. **Build the current agent** (so it gets baked in):
   ```bash
   (cd agent-rs && cargo build --release)
   test -x agent-rs/target/release/agent-client
   ```
2. **Run the base-image build** (10–20 min; full unattended install + customize):
   ```bash
   ./images/qemu/build-base-image.sh 24.04
   # options: -d/--disk-size, -r/--ram, -c/--cpus, -o/--output, -n/--dry-run
   ```
   `build_image()` installs the OS, then `virt-customize`:
   - `apt-get full-upgrade` (kernel + libraries currency = the libs/kernel/tools audit),
   - installs `socat iproute2` (vsock/transport diagnostics),
   - writes `/etc/modules-load.d/agentic-vsock.conf` = `vmw_vsock_virtio_transport`
     (guest binds the virtio-vsock device the host attaches),
   - copies `agent-client` → `/opt/agentic-sandbox/bin/agent-client`,
   - installs + enables `agent-client.service`
     (`EnvironmentFile=/etc/agentic-sandbox/agent.env`),
   - **verifies** the agent baked in (`test -x` + `systemctl is-enabled`) and
     **fails the build** if not — no silently-broken images (the #561 mode).
3. **Confirm the manifest + freeze**: the script records the qcow2 sha256 to
   `manifest.json` (provision-vm.sh verifies the backing file) and freezes the
   image read-only (`chmod 444` + `chattr +i`).
4. **Spot-check the image** (optional, non-destructive):
   ```bash
   virt-customize -a /mnt/ops/base-images/ubuntu-server-24.04-agent.qcow2 \
     --run-command 'test -x /opt/agentic-sandbox/bin/agent-client && \
       systemctl is-enabled agent-client.service && \
       grep -q vmw_vsock_virtio_transport /etc/modules-load.d/agentic-vsock.conf'
   ```

## Audit checklist (libs / kernel / tools)

- [ ] Kernel + libraries refreshed (`apt full-upgrade` ran at bake).
- [ ] `vmw_vsock_virtio_transport` configured to load on boot.
- [ ] `agent-client` present at `/opt/agentic-sandbox/bin/agent-client`.
- [ ] `agent-client.service` enabled, `EnvironmentFile` path correct.
- [ ] `qemu-guest-agent` enabled.
- [ ] Image sha256 recorded in `manifest.json`; image frozen read-only.

## Downstream / not covered here

The image is necessary but not sufficient for #561. Provisioning must also
attach the libvirt `<vsock>` device + allocate/register a per-VM CID and select
the vsock transport (no bootstrap token for host-created VMs). Those provisioning
units live in `.aiwg/planning/vm-vsock-transport-implementation.md` (Units A–E).

## References

- `images/qemu/build-base-image.sh` — the build implementation
- `.aiwg/planning/vm-vsock-transport-implementation.md` — VM vsock plan (#561)
- `.aiwg/architecture/adr/ADR-023-transport-per-runtime-security.md`
- `.aiwg/architecture/adr/ADR-026-enrollment-and-secret-retirement.md`
- `agent-rs/systemd/agent-client.service` — the baked unit
- Issue #561 (root cause + plan), #404 (transport epic)
