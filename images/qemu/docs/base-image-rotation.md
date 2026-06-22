# Base Image Rotation Procedure

Per issue #258. Ensures every Ubuntu base image consumed by `provision-vm.sh` has a known-good integrity anchor.

## Overview

Two integrity layers:

1. **ISO pin** (`images/qemu/iso-pins.json`) — GPG-verified sha256 of each pinned Ubuntu point release.
2. **qcow2 manifest** (`/mnt/ops/base-images/manifest.json`) — sha256 of each built base image, emitted by `build-base-image.sh` after a successful build.

`build-base-image.sh` checks the ISO before building. `provision-vm.sh` (via `cloud-init/common.sh:create_overlay_disk`) checks the qcow2 before creating an overlay. Both fail closed unless `AIWG_SKIP_BASE_VERIFY=1` is set. Provisioning also records the consumed base image sha256, cloud-init seed ISO sha256, and loadout manifest hashes in each VM's `vm-info.json`.

The qcow2 sanity check is virtual-size aware. Raw file length below the default threshold is treated as suspicious, but if `qemu-img info` confirms qcow2 format and a sane virtual size, the verifier continues to manifest and sha256 validation. Manifest size or sha mismatches still abort provisioning. When verification fails, the script emits `stat`, `ls`, `qemu-img info`, mount, and manifest context so CI logs can distinguish a stale path view from a bad image.

## VM Provenance Metadata

Every successful provision writes `provenance` fields to `/var/lib/agentic-sandbox/vms/<name>/vm-info.json`:

- `base_image.path`, `base_image.sha256`, and `base_image.manifest`
- `cloud_init_seed.path`, `cloud_init_seed.sha256`, `cloud_init_seed.mode`, and `cloud_init_seed.source_dir_retained`
- `loadout.path`, `loadout.source_sha256`, and `loadout.resolved_sha256` when `--loadout` is used

The cloud-init source directory is removed after the seed ISO is packed so bootstrap values are not duplicated on disk. The ISO itself remains as the libvirt seed device and is restricted to owner plus the libvirt qemu group.

## When to Rotate

- Ubuntu publishes a point release (e.g., 24.04.3 → 24.04.4).
- Security advisory affects the base image kernel or userspace.
- Quarterly hygiene refresh (recommended cadence).

## Steps

### 1. Pin the new ISO

```bash
cd images/qemu
./scripts/pin-iso.sh 24.04   # version major, not point release
```

The script:
1. Fetches `SHA256SUMS` and `SHA256SUMS.gpg` from `releases.ubuntu.com/<version>/`.
2. Verifies the signature against the pinned Ubuntu archive GPG fingerprint (`843938DF228D22F7B3742BC0D94AA3F0EFE21092`).
3. Extracts the sha256 of `ubuntu-${point_release}-live-server-amd64.iso`.
4. Writes the sha256 and current timestamp back into `iso-pins.json`.

Commit the diff with `chore(iso): pin Ubuntu <point>`.

### 2. Download the new ISO

```bash
cd /mnt/ops/isos/linux
curl -fsSLO https://releases.ubuntu.com/24.04/ubuntu-24.04.4-live-server-amd64.iso
```

### 3. Build the new base image

```bash
cd images/qemu
./build-base-image.sh 24.04
```

The build:
1. Verifies the local ISO against the pinned sha256 (fails before `virt-install` if mismatched).
2. Runs the unattended Ubuntu autoinstall.
3. Post-processes with `virt-customize` and `virt-sparsify`.
4. Records the new qcow2 sha256 into `/mnt/ops/base-images/manifest.json`.

### 4. Verify

```bash
# Confirm manifest was updated
jq . /mnt/ops/base-images/manifest.json

# Smoke-test a provision against the new image
./provision-vm.sh agent-99 --profile agentic-dev --start --dry-run
```

### 5. Reprovision running agents

The new manifest entry takes effect on the next overlay creation. Active VMs continue running their existing overlays unmodified; only re-provisioning consumes the new base.

## Emergency Bypass

If verification is broken (e.g., the manifest is stale relative to a hand-built image), provisioning can be unblocked with:

```bash
AIWG_SKIP_BASE_VERIFY=1 ./provision-vm.sh agent-99 ...
```

This is a one-time escape hatch. The bypass logs loudly to stderr and is not intended for routine use. Investigate and either re-run `pin-iso.sh` or bootstrap the manifest:

```bash
source images/qemu/lib/verify.sh
record_qcow2_manifest /mnt/ops/base-images/ubuntu-server-24.04-agent.qcow2
```

## Threat Model

Single-host local-only deployment. The integrity chain protects against:

- Local tampering with `/mnt/ops/base-images/*.qcow2` (the issue noted `chattr +i` provides write-protection but not detection; the manifest adds detection).
- Silent point-release upgrades that bypass review.
- A subverted ISO mirror feeding bad bytes into `virt-install`.

The chain does **not** protect against:

- An attacker who can rewrite both the qcow2 *and* `manifest.json` (filesystem-level compromise — out of scope).
- A subverted GPG keyring (operator must protect their own keyring; the script does best-effort `--recv-keys` but won't override an already-imported bad key).
- A host-local attacker with sufficient privileges to read the retained cloud-init seed ISO before bootstrap-sensitive values expire.

## References

- Issue #258
- `images/qemu/iso-pins.json` — pin manifest
- `images/qemu/lib/verify.sh` — verification helpers
- `images/qemu/scripts/pin-iso.sh` — pinning helper
- NIST SP 800-147 §3.1 — firmware/image authenticity
- NIST SP 800-193 §3.2 — platform integrity verification
