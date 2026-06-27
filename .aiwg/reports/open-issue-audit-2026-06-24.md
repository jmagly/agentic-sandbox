# Open Issue Audit - 2026-06-24

Scope: Gitea `roctinam/agentic-sandbox` tracker, focused pass on #561 transport regression chain and follow-on closeout work.

Mode: audit + implementation-readiness

## Snapshot (as of 2026-06-24)

- Open issues in focus: 15
- Confirmed closed: #559, #560, #562, #563
- Current root blocker: #561
- Adjacent implementation blocker: #569

## Closed with evidence

| Issue | Result | Evidence |
| --- | --- | --- |
| #559 | Closed | Issue body shows bare loadout resolution bug fixed; commit `17767ba` landed and issue `closed_at` set to 2026-06-23T16:41:15-04:00. |
| #560 | Closed | Idempotent destroy/reconcile landing confirmed by issue closure and commit history (`11bc902` / `b45e8d8`) with `closed_at` at 2026-06-23T17:20:11-04:00. |
| #562 | Closed | Issue body + tracker state show closure at 2026-06-23T17:20:11-04:00 (e2e VM reap cleanup path). |
| #563 | Closed (local) | `admin_v2` lifecycle handlers now resolve domains via candidate names/instance context for get/start/stop/destroy/restart consistency, eliminating ID-only path misses. |

## Open issues still driving #561

| Issue | Current State | Why it remains open | Immediate dependency impact |
| --- | --- | --- | --- |
| #561 | Open | Core user-facing blocker: qemu VM still does not reach stable in-guest agent enrollment state end-to-end. | Primary parent issue. |
| #569 | Open | Provision-time `AGENT_GRPC_VSOCK_CID`/`PORT` + CID map wiring still requires explicit work. | Required transport target handoff. |
| #570 | Open | Cloud-init secure transport validation still not accepting `AGENT_GRPC_VSOCK_*` tuple. | Required to avoid false fallbacks/validation reject. |
| #571 | Open | libvirt `<vsock>` path has not been fully validated in qemu provisioning flow. | Required for host-guest transport channel existence. |
| #572 | Open | Missing end-to-end qemu secure transport test for `AGENT_GRPC_VSOCK_CID/PORT`. | Required release confidence for #561 completion. |
| #573 | Open | Agent-client path divergence between base image and provisioning/lifecycle deploy path unresolved. | Required deterministic enrollment/runtime behavior parity. |
| #574 | Open | CID identity lifecycle registration/unregistration for runtime map not finalized. | Required post-provision cleanup correctness. |
| #575 | Open | Destroy/reap cleanup does not yet enforce vsock registry cleanup semantics. | Required to avoid stale CID mappings and map drift. |
| #576 | Closed | Follow-up reap cleanup + teardown behavior already closed and no longer blocks this pass. | Already accounted. |
| #577 | Open | Dynamic host-side `AGENTIC_GRPC_VSOCK_CID_MAP` via helper + reload not complete. | Required to keep map in sync at provision/destruct time. |
| #578 | Open | Base image hardening for AF_VSOCK/kernel/module/tools is incomplete. | Required for reliable guest transport stack. |
| #579 | Open | No startup/runtime consistency checks for CID map state. | Required safety net to prevent stale mappings. |
| #580 | Open | Docs/AIWG flow and map reload + teardown signaling not yet updated. | Required operability and runbook completeness. |
| #581 | Open | Concurrent `.vsock-cidr-registry` write race window still unresolved. | Required under parallel provisioning pressure. |
| #582 | Open | `AGENTIC_GRPC_VSOCK_PORT` not yet enabled in default/dev runtime startup by default. | Required for host listener availability. |
| #583 | Open | Startup validation of `AGENTIC_GRPC_VSOCK_CID_MAP` entries is not implemented. | Required hardening and startup fail-fast. |
## Recommended immediate execution order

1. `#578` + `#571` + `#569` (guest transport channel and CID handoff readiness).
2. `#570` + `#582` + `#577` + `#583` (runtime/provisioning + listener/map integrity and env emission).
3. `#574` + `#575` + `#579` + `#581` (mapping lifecycle and concurrency safety).
4. `#573` + `#572` + `#580` (path reconciliation, verification coverage, and operator documentation).

## Suggested `address-issues` next command

```bash
address the open issues 569 570 571 572 573 574 575 577 578 579 580 581 582 583
```

(Use `#561` as the acceptance gate once dependencies are addressed.)
