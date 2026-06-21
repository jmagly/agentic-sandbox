# Legacy Agent Secret Inventory

Date: 2026-06-20

Issues: #536, #412, #507

## Summary

Legacy `AGENT_SECRET`, `x-agent-secret`, and TOFU-style first-connect agent
authentication are retired. New managed runtimes must use UDS, vsock, mTLS, or
bootstrap enrollment that materializes mTLS credentials. Remaining references
are either fail-closed code paths, tests proving retirement, historical release
notes, or migration documentation linked to #412.

## Disposition Table

| Area | References | Disposition | Evidence |
| --- | --- | --- | --- |
| Agent gRPC auth | `management/src/grpc.rs` | Keep fail-closed tests | Legacy-only metadata returns unauthenticated; transport identity wins even when stale `x-agent-secret` metadata is present. |
| Agent metadata | `agent-rs/src/main.rs` | Keep omission tests | Tests assert TCP and secure transports do not send `x-agent-secret`. |
| VM provisioning | `images/qemu/provision-vm.sh`, `images/qemu/cloud-init/common.sh`, `images/qemu/loadouts/generate-from-manifest.sh` | Keep fail-closed retirement checks | Scripts reject legacy secret fallback and require secure transport material/bootstrap enrollment. |
| Container provisioning | `images/container/agent-entrypoint.sh`, `management/src/http/containers.rs` | Keep fail-closed retirement checks | Containers reject `AGENT_SECRET` and require secure transport env or bootstrap enrollment. |
| Agent deploy scripts | `scripts/deploy-agent.sh`, `deploy/install-agent.sh`, `scripts/provision-vm-agent.sh` | Normalize/fail closed | `deploy-agent.sh` now refuses VMs with `AGENT_SECRET` and writes a systemd unit that reads `/etc/agentic-sandbox/agent.env`; install/provision helpers reject legacy `--secret` / `AGENT_SECRET`. |
| Admin API | `management/src/http/admin_v2.rs`, `management/src/http/server.rs`, `docs/contracts/admin-api.openapi.yaml` | Keep retired endpoint | Secret rotation returns `410 Gone` and points operators to transport identity credentials. |
| Current docs | `BUILD.md`, `deploy/README.md`, `agent-rs/README.md`, `management/README.md`, `docs/API.md`, `docs/grpc-architecture.md`, `docs/security/attack-surface.md`, `docs/DEPLOYMENT.md`, `docs/OPERATIONS.md`, `docs/TROUBLESHOOTING.md` | Mark retired or fail-closed | Current docs describe legacy secret auth as retired/rejected and link behavior to #412 where relevant. |
| Tests | `images/qemu/tests/*`, `images/qemu/loadouts/tests/*`, `tests/container/*` | Keep | Fixtures intentionally contain fake `AGENT_SECRET` values to prove output omits or rejects them. |
| Historical docs | `CHANGELOG.md`, `docs/releases/*`, rollout plans | Keep as history/migration context | Historical references are not current deployment guidance. The active rollout plan still records old phases but #412/#536 own cleanup state. |

## Current Posture

- Runtime provisioning docs do not instruct new managed deployments to use
  `AGENT_SECRET`.
- Active deploy/provision entrypoints reject legacy secret material instead of
  consuming it.
- Plain TCP remains an explicit unsafe/dev transport mode with no agent
  authentication metadata path; the server rejects non-loopback plaintext
  management TCP unless `AGENTIC_ALLOW_PLAINTEXT_TCP=1` is set.
- Remaining `AGENT_SECRET` literals in tests are expected negative fixtures.

## Verification

- `bash -n scripts/deploy-agent.sh scripts/provision-vm-agent.sh images/container/agent-entrypoint.sh images/qemu/provision-vm.sh images/qemu/loadouts/generate-from-manifest.sh`
- `rg -n -e "Found secret" -e "Reading secret" -e "--secret" -e "AGENT_SECRET" scripts deploy images agent-rs management/src docs README.md BUILD.md -S --glob '!**/target/**' --glob '!**/tests/**' --glob '!docs/releases/**' --glob '!CHANGELOG.md'`

