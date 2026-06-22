# Doc Sync Audit - 2026-06-22 Release

## Summary

- Direction: `code-to-docs`
- Scope: incremental release audit from `v2026.6.28` to `HEAD`
- Trigger: release gate `doc-sync-code-to-docs`
- Target release: `v2026.6.29`
- Result: PASS after documentation updates in this release-prep pass

The audited code surface covers local CA rotation, operator-auth evidence,
dashboard CSP/DOM-sink hardening, admin-v2 Docker readiness signaling, dev-mode
plaintext bind guidance, QEMU provision script resolution, VM provisioning
provenance, and open-issue release blocker evidence.

## Code Surface Audited

- `management/src/grpc_local_ca.rs`
- `management/src/http/operator_auth.rs`
- `management/src/http/server.rs`
- `management/ui/app.js`
- `management/ui/index.html`
- `management/src/http/admin_v2.rs`
- `management/src/grpc.rs`
- `management/dev.sh`
- `images/qemu/provision-vm.sh`
- `.gitea/workflows/ci.yaml`

## Documentation Surface Audited

- `CHANGELOG.md`
- `docs/releases/`
- `docs/contracts/admin-api.openapi.yaml`
- `docs/container-runtime.md`
- `docs/tui-orchestration-support.md`
- `docs/getting-started.md`
- `management/README.md`
- `docs/security/security-status.md`
- `docs/security/attack-surface.md`
- `docs/security/standards-alignment.md`
- `docs/security/asvs-profile.md`
- `docs/releases/verification.md`
- `images/qemu/README.md`
- `images/qemu/docs/base-image-rotation.md`

## Findings

### DOC-DRIFT-001 - Admin v2 Docker readiness fields

Status: fixed.

`Instance` responses gained `agent_registered`, `agent_ready`, and
`container_finished_at`, and Docker operation status now distinguishes
`bootstrap_pending` and `not_ready`. The OpenAPI schema, container runtime
guide, TUI support runbook, changelog, and release notes now describe these
fields and the consumer rule: do not infer session readiness from runtime or
AgentCard metadata alone.

### DOC-DRIFT-002 - Docker-reachable dev bind policy

Status: already fixed before report.

`management/dev.sh`, `management/README.md`, and `docs/getting-started.md`
already document the explicit `AGENTIC_ALLOW_PLAINTEXT_TCP=1` acknowledgement
required for non-loopback Docker dev binds. The release notes now include the
operator impact.

### DOC-DRIFT-003 - QEMU provision script resolution

Status: fixed.

Admin v2 now resolves `images/qemu/provision-vm.sh` from the stable checkout
root, honors `AIWG_PROVISION_VM_SCRIPT`, and reports attempted paths on spawn
failure. Release notes now include this behavior and the operator override.

### DOC-DRIFT-004 - Dashboard CSP posture

Status: fixed.

Security posture docs still described dashboard CSP/XSS hardening as future
work. The attack surface inventory, standards alignment, security status page,
changelog, and release notes now reflect the current local-dashboard evidence
while preserving the non-claim for complete remote multi-user admin hardening.

### DOC-DRIFT-005 - Release notes and changelog

Status: fixed.

The stable-release gate requires a changelog entry and release notes for
`2026.6.29`. Added both, including verification commands and operator notes.

## Validation

- `bash -n management/dev.sh`
- `cargo test provision_vm_script --lib`
- `cargo test provision_vm_spawn_error --lib`
- `cargo test docker --lib`
- `cargo test ready_heartbeat_marks_preregistered_context_ready_without_replacing_it --lib`
- `cargo fmt --check`
- `git diff --check`

## Manual Review

No unresolved user-facing documentation drift remains for the audited
post-`v2026.6.28` release surface. Historical `.aiwg/` planning artifacts were
not rewritten except where this release prep adds current evidence.
