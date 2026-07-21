# Libvirt checkpoint and warm-pool operations

## Purpose

Operate the QEMU/q35 fast-resume path delivered by `checkpoint-vm.sh` and the
v2 management API. This path targets restore-to-usable latency in seconds; the
Cloud Hypervisor path remains the sub-second option.

## Scope

The workflow captures only pre-enrollment bases. It detaches virtiofs before
`virsh save`, persists UEFI NVRAM, and creates a fresh disk overlay, NVRAM,
vsock CID, bootstrap token, and mTLS identity for every restore or
warm-pool handoff. Libvirt requires a saved state to retain both its domain
name and UUID, so checkpoint creation undefines the stopped source and each
warm slot is backed by a distinct pre-booted checkpoint that is consumed once.

## Prerequisites

- q35/UEFI libvirt domain with QEMU guest agent, vsock, and the
  `agent-client-restore-bootstrap` units installed.
- Isolated `agentinbox` virtiofs mount and writable VM/IP/CID registries.
- Management server bootstrap-token store and gRPC CA backend enabled.
- `virsh`, `qemu-img`, `jq`, `flock`, and access to `qemu:///system`.
- A clean base that has never received tenant credentials. Clean-base
  verification rejects active agent processes, TLS material, bootstrap data,
  and token-bearing agent environment files.

## Procedure

1. Boot the clean base and create a named checkpoint through
   `POST /api/v2/admin/libvirt/checkpoints` with `pre_enrollment: true`.
2. Poll the operation URL until it succeeds. The source domain is saved,
   stopped, and undefined; its name and UUID now belong to the checkpoint.
3. Either restore it directly, or capture 1–64 distinct clean bases and
   initialize a pool with their checkpoint ids.
4. Hand out a slot. The slot retains its saved libvirt name/UUID while the
   management server generates a fresh canonical tenant instance id, CID, and
   one-time enrollment envelope.
5. Poll the operation. Success requires both the guest's mode-0600 enrollment
   acknowledgement and the corresponding mTLS registry identity.

## Verification

- Operation result reports a fresh `instance_id`, `vsock_cid`, IP address,
  restore duration, `bootstrap_token_issued: true`, and
  `authenticated_enrollment_ready: true` without a raw token.
- `$VM_STORAGE_DIR/<name>/enroll-on-restore.json` records the non-secret
  handoff metadata.
- The guest inbox no longer contains `restore-bootstrap.env`; its acknowledgement
  names the expected SPIFFE identity and a SHA-256 certificate fingerprint.
- Titan CI runs `checkpoint-vm.sh selftest --latency-budget-ms 10000` on the
  serialized libvirt lane and retains the measured default-profile result.

## Rollback

If restore fails before handoff, the wrapper removes only the newly allocated
domain, VM directory, inbox, DHCP row, IP row, and CID row. A failed warm
handoff does not consume its slot. To abandon an unused pool, stop using its
API name and remove its exact directory only during an approved maintenance
window.

## Troubleshooting

- `virtiofs ... LOG_SHMFD`: confirm the devices were live-detached before save.
- `fresh restore requires an attested pre-enrollment checkpoint`: recreate the
  checkpoint through the management path; do not relabel secret-bearing RAM.
- CID/IP allocation failure: inspect the locked registries for a genuinely
  active owner before changing entries.
- Enrollment timeout: check the guest restore-bootstrap path/timer, isolated
  inbox attachment, HTTPS enrollment route, and management mTLS registry.
- Latency failure: preserve the emitted measurement and inspect host I/O and
  memory pressure; do not raise the budget without an acceptance review.

## Evidence

Retain the operation JSON, checkpoint metadata (not the RAM image), Titan CI
run URL, and latency JSON. Checkpoint RAM, NVRAM, bootstrap drops, CA material,
SSH keys, and token-store files are sensitive host artifacts and must not be
uploaded or pasted into issue comments.
