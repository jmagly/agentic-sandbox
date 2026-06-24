# VM vsock Transport — Implementation Plan (#561)

**Status**: Draft for review
**Anchors**: ADR-023 (transport-per-runtime), ADR-024 (unified SPIFFE identity), ADR-026 (zero-touch enrollment), `.aiwg/planning/agent-transport-security-rollout.md` (Phase 2), spike-005 (native AF_VSOCK + tonic), #561, transport epic #404

## TL;DR

There is **no new architectural decision** here. ADR-023 already assigns the
**VM (same host)** runtime to **gRPC over vsock**, identity = **host-assigned
CID**, **no certs, no token**; ADR-026 makes host-created VMs *local builds*
with **implicit host-mediated enrollment** ("nothing to enroll"). #561 is the
**Phase-2 implementation gap**: provisioning still wires same-host qemu VMs onto
the *remote/fleet* path (in-VM keygen + single-use bootstrap token + mTLS-TCP,
HTTP enrollment), which then fails because the bootstrap-enrollment endpoint is
loopback-bound (`127.0.0.1:8122`) and unreachable from the VM. The fix is to
select the prescribed vsock path for same-host VMs.

## Current state (what already exists)

| Component | State | Evidence |
|-----------|-------|----------|
| Agent vsock dial | ✅ implemented | `agent-rs/src/main.rs` `TransportMode::Vsock`, `TonicVsockIo`, `tokio_vsock` |
| Mgmt vsock gRPC listener | ✅ implemented | `management/src/main.rs:1369` `serve_grpc_vsock` binds `VMADDR_CID_ANY:port` |
| Mgmt UDS gRPC listener | ✅ implemented | `main.rs:1342` `serve_grpc_uds` (Firecracker/vhost-device-vsock bridge path) |
| CID→instance identity map | ✅ implemented | `management/src/transport_identity.rs` `register_vsock_cid`, `VsockCid` peer evidence |
| Native AF_VSOCK + tonic | ✅ spiked | `.aiwg/spikes/spike-005-native-vsock-tonic.md` (#406) |
| mTLS-TCP fallback | ✅ implemented | `main.rs:1387` `serve_grpc_mtls` (SPIFFE URI-SAN) |

## The gap (what's missing)

1. **No `<vsock>` device in the libvirt domain XML.** `images/qemu/provision-vm.sh`
   and `backends/libvirt.sh` emit no virtio-vsock device, so the guest has no
   vsock channel to the host regardless of the host listener. **Core blocker.**
2. **No per-VM CID allocation.** ADR-023 guardrail: "vsock port fixed; CID
   assigned at VM create and recorded." Nothing allocates a guest CID or calls
   `register_vsock_cid(cid → instance_id)`.
3. **Provisioning doesn't select vsock for same-host VMs.** The mgmt v2 qemu
   branch always issues the fleet bootstrap token + writes
   `AGENT_BOOTSTRAP_ENROLLMENT_URL` (HTTP) into `agent.env`, instead of writing
   the vsock transport selection. (`admin_v2.rs:1336` `vm_bootstrap_enrollment_url()`.)
4. **The host vsock listener may not be launched in the default/dev config.**
   Confirm `serve_grpc_vsock` is started (and on the fixed port) when VMs are
   provisioned; today only TCP gRPC (8120) + mTLS-TCP (8123) are observed bound.
5. **No agent-side transport directive for VMs.** `agent.env` needs to tell the
   agent to dial vsock (host CID = 2, fixed port) rather than run bootstrap
   enrollment.

## Implementation units

Sequenced; agent-oriented scope (not wall-clock). Units B/C/D are parallel-ready
once A lands.

**Unit A — libvirt vsock device + CID allocation (foundation).**
- Add `<vsock model='virtio'><cid auto='no' address='N'/></vsock>` to the domain
  XML in `provision-vm.sh` / `backends/libvirt.sh`.
- Allocate a unique guest CID per VM (CID ≥ 3; 0/1/2 reserved) from a host-side
  registry alongside the existing IP registry; record it in `vm-info.json`.
- Verify-gate: domain boots with the vsock device; `vsock` kernel module present
  in the base image (add to base image if absent — see Unit F).

**Unit B — host vsock listener wiring.**
- Ensure `serve_grpc_vsock(port)` is launched in the management runtime on the
  fixed vsock port; bind on the host (CID = `VMADDR_CID_HOST` = 2).
- On a guest gRPC connection, resolve peer guest-CID → instance via
  `transport_identity` (`VsockCid` evidence); reject unmapped CIDs (closes TOFU,
  per ADR-026 §"unknown identity ⇒ reject").

**Unit C — provisioning transport selection (same-host VM ⇒ vsock).**
- In the mgmt v2 qemu branch, for same-host VMs select transport = vsock:
  - Register the allocated guest CID → instance_id before/at start.
  - Write the vsock directive into `agent.env`
    (`MANAGEMENT_TRANSPORT=vsock`, host CID = 2, fixed `VSOCK_PORT`), and **omit**
    the bootstrap token + `AGENT_BOOTSTRAP_ENROLLMENT_URL` (host-mediated, no
    enrollment per ADR-026).
- Keep the fleet token path only for genuinely remote agents.

**Unit D — agent transport directive consumption.**
- Have the agent honor the vsock directive from `agent.env`
  (`--transport vsock --vsock-cid 2 --vsock-port P`) and skip bootstrap
  enrollment when host-mediated identity is in effect. (`TransportMode::Vsock`
  exists; wire env → flags in the agent-client launch / service unit.)

**Unit E — fallback ladder + dual-mode.**
- Implement ADR-023's `auto` ladder for VMs: vsock → mTLS-TCP fallback when
  vsock is unavailable (R-7), recording the chosen transport per agent.
- Note: the mTLS-TCP fallback still needs the enrollment endpoint reachable
  (the original #561 Layer-3 concern). Track separately; the vsock primary path
  makes it non-blocking for the common case.

**Unit F — base image: ensure vsock + current agent.**
- Refresh the qemu base image (currently 2026-01-25, no current agent) so it
  ships the `vsock`/`vhost_vsock` guest modules and the modern `agent-client` +
  self-enrolling service unit — eliminates the per-provision deploy (parallels
  docker). Update `build-base-image.sh`. (Also resolves #561 Layer-1.)

**Unit G — tests.**
- Extend `transport_identity` coverage (a CID-normalization test already exists).
- E2E: provision a qemu VM with a vsock device → agent dials vsock → host
  resolves CID → gRPC session established, **no token, no HTTP enrollment**.

## Risks

| Risk | Mitigation |
|------|------------|
| vsock module absent in base image (R-7) | Unit F bakes it in; Unit E falls back to mTLS-TCP |
| CID collision across concurrent VMs | Host-side CID registry (Unit A), allocate-and-record like IP registry |
| Unmapped/forged guest CID | Reject unmapped CIDs (Unit B); CID is hypervisor-assigned (stronger than a bearer token) |
| mTLS-TCP fallback still loopback-unreachable for enrollment | Tracked as the residual #561 Layer-3 item; not on the vsock path |

## Why this is the right call

vsock gives **host↔guest-only** isolation (no reachable IP port to scan, DoS,
or spoof — ADR-023 §Insight), and CID identity is **hypervisor-attested** at VM
birth — so there is literally nothing to enroll (ADR-026 §"Local build"). It is
the same control-plane model Firecracker / Kata / AWS Nitro use. It also keeps
the operator-surface loopback posture (#256/#257) fully intact — no enrollment
endpoint is exposed to any network at all.

## References

- @.aiwg/architecture/adr/ADR-023-transport-per-runtime-security.md
- @.aiwg/architecture/adr/ADR-024-unified-spiffe-identity.md
- @.aiwg/architecture/adr/ADR-026-enrollment-and-secret-retirement.md
- @.aiwg/planning/agent-transport-security-rollout.md
- @.aiwg/spikes/spike-005-native-vsock-tonic.md
- @management/src/main.rs (serve_grpc_vsock / serve_grpc_uds / serve_grpc_mtls)
- @management/src/transport_identity.rs
- #561 (this gap), #404 (transport epic), #406 (vsock spike)
