# Monthly Report - 2026-05

Backfilled: 2026-07-08  
Scope: May 2026

## Summary

May was the first heavily recorded release month. It included the v2/A2A
contract push, release-pipeline hardening, security remediation, VM/loadout
expansion, PTY/TUI work, doc-sync audits, and CalVer releases from `2026.5.0`
through `2026.5.17`.

## Evidence Reviewed

- `CHANGELOG.md` sections `2026.5.0` through `2026.5.17`.
- `docs/releases/v2026.5.1.md` through `docs/releases/v2026.5.17.md`.
- `.aiwg/reports/doc-sync-20260508T191917Z.md`,
  `.aiwg/reports/doc-sync-20260509T071322Z.md`, and
  `.aiwg/reports/doc-sync-20260524T190301Z.md`.
- `.aiwg/security/audit-2026-05-15/SUMMARY.md` and issue files under
  `.aiwg/security/audit-2026-05-15/issues/`.
- `.aiwg/architecture/adr/ADR-006-schema-first-authoring.md` through
  `.aiwg/architecture/adr/ADR-022-three-surface-architecture.md` for the v2
  executor contract wave.
- `.aiwg/architecture/adr/ADR-023-transport-per-runtime-security.md` through
  `.aiwg/architecture/adr/ADR-027-cert-lifecycle-and-hot-reload.md` for agent
  transport security planning.

## Delivered

- v2/A2A executor contract work: REST handlers, PTY WebSocket binding,
  AgentCard routing, push delivery, runtime/idempotency/HITL/multi-tenant
  extensions, admin migration, and compatibility shims.
- Security and supply-chain hardening: token redaction, constant-time secret
  verification, cloud-init and virtiofs permission tightening, Dockerfile and
  CI pinning, npm global-install pinning, base image provenance checks, ISO and
  qcow2 verification, and AGPL licensing adoption.
- Release infrastructure: version bump script, release runbook, binary
  tarballs, checksums, Linux package smoke paths, tag gating, GHCR/internal
  registry publication work, and aarch64 build planning.
- VM/loadout work: browser QA profile, automation-control loadout, Codex
  automation launcher, current-agent readiness enforcement, compressed qcow2
  verification, and live-VM validation helpers.
- PTY/TUI/session work: pty-ws v1 client migration, transcript/archive
  handling, screen-state redraw stabilization, TUI stress harness, event memory
  metrics, output aggregator backpressure metrics, and VM event bridge.
- Agent transport security planning and spikes: ADRs, requirements, risks,
  threat model, rollout plan, native vsock tonic spike, and rustls hot reload
  spike.

## Gaps And Carryover

- Early May releases were source-only while release asset automation matured.
- Some release/E2E gates were explicitly adjusted around runner constraints and
  base-image availability.
- Security posture work continued into June for transport and credential
  hardening.

## Verification Snapshot

Checked-in May evidence includes:

- `doc-sync-20260508T191917Z.md`: 33 findings, 15 auto-fixed.
- `doc-sync-20260524T190301Z.md`: 10 findings, all auto-fixed.
- `CHANGELOG.md`: release entries from `2026.5.0` to `2026.5.17`.
- Security audit dated 2026-05-15 with 11 issue files.

