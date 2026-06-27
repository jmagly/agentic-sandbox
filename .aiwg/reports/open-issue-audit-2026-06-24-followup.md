# Open Issue Audit Follow-up - 2026-06-24

Scope: Gitea `roctinam/agentic-sandbox` tracker, transport-chain corrective workstream after #561 triage.

## Authoritative issue snapshot (verified via `mcp__git_gitea.issue_read`)

### Confirmed closed

- #559: fix(vm): provision-vm.sh does not resolve bare loadout names
- #560: runtime: DELETE instance returns 404 when backing VM removed out-of-band
- #562: interrupted e2e runs leak VMs; reap not run on teardown
- #564: docs: document vsock CID registry and VM env outputs
- #565: test: add coverage for CID allocation + vsock CID cleanup lifecycle
- #566: refactor: reconcile vsock_cid argument flow across provisioning entrypoints
- #567: bug: reclaim vsock CID on failed/non-finalized VM provisioning
- #568: test: wire vsock CID lifecycle checks into automated CI target
- #576: bug(scripts): avoid side effects when sourcing scripts/reap-e2e-vms.sh
- #563: v2 lifecycle resolves qemu domains by ID mismatch (get/start/stop/restart) now fixed locally pending tracker closure

### Confirmed open

- #561: qemu VM provisions to running but in-guest agent never enrolls
- #569: provision-time AGENT_GRPC_VSOCK_CID/PORT and CID map wiring
- #570: cloud-init secure transport validation should recognize AGENT_GRPC_VSOCK_* tuple
- #571: Unit A libvirt `<vsock>` injection in provisioning flow
- #572: end-to-end qemu secure transport test for AGENT_GRPC_VSOCK_*
- #573: reconcile agent-client install path between image and provisioning/live deploy
- #574: runtime AGENTIC_GRPC_VSOCK_CID_MAP lifecycle registration/unregistration
- #575: enforce vsock registry cleanup in destroy/reap
- #577: dynamic AGENTIC_GRPC_VSOCK_CID_MAP via host helper + reload
- #578: harden base image for AF_VSOCK and transport tools
- #579: lifecycle consistency checks for AGENTIC_GRPC_VSOCK_CID_MAP state
- #580: update docs and AIWG flow for map reload/teardown signaling
- #581: mitigate concurrent `.vsock-cidr-registry` writes
- #582: enable AGENTIC_GRPC_VSOCK_PORT in default/dev runtime startup
- #583: validate AGENTIC_GRPC_VSOCK_CID_MAP entries at startup

## Immediate sequencing used for this lane

1. Unit A and provisioning handoff: #571, #569, #570, #582, #577
2. Runtime cleanup/consistency: #574, #575, #579, #581, #580, #583
3. Validation + docs/path parity: #572, #573, #578

## Recommended next command chain

From the current state, the next flow is:

```text
address the open issues 569 570 571 572 573 574 575 577 578 579 580 581 582 583
```

Use `#561` as the final acceptance gate after dependency chain completion.
