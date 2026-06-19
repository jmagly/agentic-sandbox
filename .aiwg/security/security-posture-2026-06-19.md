# Security posture matrix for launch review

Date: 2026-06-19

Scope: reconciliation of the 2026-05-15 audit findings into the current launch
posture for agentic-sandbox.

This document is the closure matrix for Gitea issue #504. It does not replace
the original audit packet under `.aiwg/security/audit-2026-05-15/`; it records
the current decision state, evidence, remaining gaps, and launch claim language.

## Status legend

| Status | Meaning |
| --- | --- |
| Fixed | Current code or deployment assets appear to address the finding. |
| Partial | The main risk is reduced, but evidence, docs, or secondary surfaces remain. |
| Open | The finding still needs engineering or verification before a strong claim. |
| Superseded | The vulnerable mechanism was removed or replaced by a different design. |
| Accepted | The risk is intentionally deferred with explicit claim limits. |

## Launch posture summary

| Claim area | Launch posture | Boundary for external wording |
| --- | --- | --- |
| Agent control transport | Qualified | The project can claim transport identity support for UDS, vsock, and mTLS agent connections. Do not claim every deployment is authenticated unless the release report verifies the selected deployment profile. |
| Local management API | Qualified | The default management surface is local-first. Do not claim production-grade multi-user remote admin authentication. |
| Credential non-exposure | Qualified | The project can claim metadata-only credential references and write-only credential APIs. Do not claim secrets never enter workloads; some providers require file/env materialization inside a container or VM. |
| Supply-chain provenance | Partial | Digest/action pinning and ISO verification machinery exists, but image pin evidence and release verification must be completed for launch. |
| Runtime isolation | Qualified | The platform uses VM/container isolation boundaries. Do not imply guest compromise is fully contained against all host or side-channel paths until the open mount, seclabel, and proxy gaps are closed. |

## Closure matrix

| ID | Original finding | Current posture | Evidence | Launch decision |
| --- | --- | --- | --- | --- |
| B1 | Base ISO and qcow2 backing image lacked signature/hash verification. | Partial | `images/qemu/scripts/pin-iso.sh`, `images/qemu/lib/verify.sh`, `images/qemu/docs/base-image-rotation.md`, `images/qemu/iso-pins.json`. | Verification tooling exists, but the current launch artifact set must show populated pins and recorded manifests. Track in #509. |
| B2 | Cloud-init seed ISO contained plaintext `AGENT_SECRET`. | Superseded / Partial | Legacy `AGENT_SECRET` auth retired in `docs/API.md`, `agent-rs/README.md`, and `management/src/grpc.rs`; bootstrap enrollment uses one-time token and mTLS material in `agent-rs/src/main.rs` and `images/qemu/cloud-init/common.sh`. | Shared-secret agent auth is replaced. Still qualify cloud-init bootstrap material as sensitive and verify seed ISO permissions and token scrub behavior in release evidence. |
| B3 | Dev compose mounted host `docker.sock`. | Fixed | `docker-compose.dev.yaml` removes the socket mount and points operators to a Docker socket proxy when Docker API access is required. | Safe to claim the default dev compose no longer hands the management container raw host Docker control. |
| H1 | WebSocket server accepted unauthenticated commands. | Partial | `docs/API.md` documents local-host HTTP/WebSocket access with bearer auth only for session dispatch; no general remote admin auth claim is present. | Launch may describe local operator WebSocket telemetry. Do not market remote dashboard exposure without auth hardening. Track profile-specific API controls in #510 and public claim wording in #513. |
| H2 | Plaintext transports and default `0.0.0.0` listener. | Partial | `management/src/config.rs` now defaults to `127.0.0.1:8120`; gRPC rejects plain TCP agent identity in `management/src/grpc.rs`; production compose publishes host ports on `127.0.0.1`; release-prep docs now list loopback defaults and secure side channels. Some deployment assets intentionally listen on all container interfaces. | Treat as functionally reduced but profile-dependent. Release verification #507 must identify each profile's listener and transport mode. |
| H3 | Non-constant-time token comparison. | Fixed / Superseded | Bootstrap token validation uses constant-time comparison via `subtle` in `management/src/bootstrap_enrollment.rs`; old agent shared-secret path is retired. | Safe to close as a shared-secret auth finding. Keep constant-time checks in future credential stores. |
| H4 | virtiofs RW mounts missing `nodev,nosuid,noexec`. | Open | Current evidence did not prove all VM shared mounts enforce these flags. | Do not claim hardened mount policy yet. Include in launch attack-surface inventory and follow-up verification. |
| H5 | Secret directory and token file permissions too broad. | Partial | Bootstrap token store and agent mTLS key material use restricted modes in `management/src/bootstrap_enrollment.rs`, `agent-rs/src/main.rs`, and `images/qemu/cloud-init/common.sh`. | Main legacy path is replaced. Verify all remaining host secret directories and generated seed files before strong launch claims. |
| H6 | `HEALTH_TOKEN_PLACEHOLDER` literal in dev profile. | Fixed / Needs release grep | No current match was found in inspected active paths. | Run release grep as part of #508 before public release. |
| H7 | Non-atomic shared-secret rotation race. | Superseded | Rotation endpoints now return gone/retired messages in `management/src/http/server.rs` and `management/src/http/admin_v2.rs`; bootstrap enrollment is the replacement path. | Close as superseded by transport identity and one-time bootstrap enrollment. |
| H8 | Workflow actions were tag-pinned instead of SHA-pinned. | Partial | Current workflows are mostly SHA-pinned with digest rationale in `ci/digests.txt`. | Treat tag-pin risk as mostly fixed, but retain release verification for any unpinned or stale action. |
| H9 | Dockerfile `FROM` lines used floating tags. | Partial / Open | Many base images are digest-pinned, but `images/container/Dockerfile.automation-control` still uses `agentic/codex:latest` and `images/agent/claude/Dockerfile` still depends on a local latest-style base. | Do not claim complete image digest pinning until these are resolved or explicitly excluded from release. |
| H10 | `actions/upload-artifact@v3` was deprecated/EOL. | Open / Qualified | Some workflows still reference SHA-pinned v3 artifact actions. | SHA pinning reduces substitution risk, but deprecated action runtime risk remains. Include in release verification. |
| H11 | Global npm installs were unpinned. | Partial | Active Codex install paths pin `@openai/codex@0.130.0`; backups and legacy paths still show unpinned or inherited installers. | Treat current release paths as improved. Exclude backups from release artifacts or lint them separately. |
| M1 | Generated libvirt XML lacked seclabel/sVirt enforcement. | Open | No complete launch evidence found for generated `<seclabel>` policy. | Do not claim sVirt/AppArmor confinement until verified. |
| M2 | Management UI had many `innerHTML` sinks and no CSP. | Open | `management/ui/app.js` has escaping helpers and safe markdown rendering, but broad sink audit and CSP evidence remain incomplete. | Avoid claims of hardened browser UI. Track as API/UI security profile work under #510. |
| M3 | Deploy Dockerfiles ran as root. | Open / Needs verification | Not fully re-audited in this pass. | Include in release verification checklist. |
| M4 | Claude agent image used latest-style base. | Open | `images/agent/claude/Dockerfile` still needs release pin verification. | Same decision as H9. |
| M5 | Loadout provenance was not recorded in VM info. | Partial | Loadout manifest tests and base image manifest tooling exist, but launch artifact evidence remains incomplete. | Track in #509. |
| L1 | No crash-path revocation hook for VM secrets. | Open | Credential leases can be revoked through broker APIs, but crash-path enforcement was not proven. | Qualify session credential claims as lease-capable, not guaranteed crash-revoked. |
| L2 | No TPM/Secure Boot. | Accepted | This is still outside current local-first launch boundary. | State as out of scope for launch threat model. |
| L3 | Docker runtime allowed writable FS and extra caps. | Partial | Dev test compose now uses `read_only: true`, tmpfs, `no-new-privileges`, and `cap_drop: ALL`; broader runtime inventory still needs verification. | Claim only for the inspected dev test profile. |
| L4 | Mixed Rust toolchains. | Partial | Current Docker and CI paths appear more consistent, but not fully audited in this pass. | Keep in release verification, not a primary market claim. |
| L5 | No `cargo audit` in CI. | Open | No complete CI evidence was found in this pass. | Do not claim continuous dependency vulnerability gating until implemented. |
| L6 | `curl | sh` installers in backups and cloud-init profiles. | Partial / Accepted | Active release paths are improved, but legacy backups and optional profiles still carry installer risk. | Exclude legacy backups from release packaging or document them as non-release assets. |

## Documentation drift resolved in release prep

- `README.md` now lists the `LISTEN_ADDR` default as `127.0.0.1:8120`, matching
  `management/src/config.rs`.
- `docs/ARCHITECTURE.md` now presents transport identity rather than the
  retired `x-agent-secret` path in the current registration flow.
- `docs/API.md` correctly documents local-only HTTP/WebSocket behavior; public
  launch copy must still avoid collapsing that into a general "authenticated
  dashboard" claim.

## Required launch evidence

1. Run the transport verification report from #507 against each supported
   deployment profile.
2. Populate or explicitly waive current ISO and qcow2 pins in #509.
3. Complete the release verification guide in #508 for workflow actions,
   Dockerfile bases, npm installs, UI CSP, and stale secret patterns.
4. Publish `docs/security/attack-surface.md` as the stable launch inventory.
5. Publish the credential posture decision in
   `.aiwg/security/credential-posture-2026-06-19.md` and keep credential proxy
   work as implementation follow-up rather than a current launch claim.
