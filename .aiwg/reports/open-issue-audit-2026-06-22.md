# Open Issue Audit - 2026-06-22

Scope: Gitea `roctinam/agentic-sandbox`, open issues after the 2026-06-22 address-issues pass.

Mode: audit plus tracker status comments. Code changes were only made where an issue was safe and verifiable.

## Summary

- Open issues audited: 21
- Issues closed during this pass family: #363, #367, #479, #480, #509, #549, #550, #551
- Current high-priority external blockers: #478, #488, #489
- Current human-authorization gates: #306, #404, #411, #462, #481, #494, #495, #499, #507, #516, #518
- Current dependency/deferred set: #114, #118, #119, #120, #438, #489, #503, #517

## Closed With Evidence

| Issue | Result | Evidence |
| --- | --- | --- |
| #551 | Closed | Commit `7ec7cae` hardened dashboard CSP and DOM sinks. |
| #550 | Closed | Commit `d523579` exposed Docker provision readiness failures. |
| #549 | Closed | Commit `ae98435` fails early on unsafe Docker dev bind. |
| #363, #367 | Closed | Commit `4b43493` recorded accepted titan runner posture in CI/runbook docs. |
| #479 | Closed | `v2026.6.28` GitHub release assets verified: `.deb`, `.rpm`, installer, sidecars, and package checksums. |
| #480 | Closed | `scripts/verify-release-assets.sh v2026.6.28 --skip-ghcr` passed package checksum and installer dry-run checks. |
| #509 | Closed | Commit `d9c1834` records base image, cloud-init seed ISO, and loadout manifest hashes in VM metadata; docs updated. |

## Human Authorization Required

The `address-issues-threat-assess` gate flagged these issues. Do not implement them autonomously without explicit human authorization naming the issue number.

| Issue | Gate reason | Current tracker action |
| --- | --- | --- |
| #306 | GitHub PAT / release-mirror secret path. | Gate note posted. |
| #404 | Legacy shared-secret/TOFU, bootstrap token, CA/private-root material surfaces. | Gate note posted. |
| #411 | CA private material / OS secrets/keychain scope. | Gate note posted. |
| #462 | Secret-dependent release path context. | Gate note posted. |
| #481 | `MUTSU_SSH_KEY` and mutsu SSH authentication path. | Gate note posted. |
| #494 | Linux keyring backend for local CA private material. | Gate note posted. |
| #495 | macOS Keychain backend for local CA private material. | Gate note posted. |
| #499 | Provider CLI auth-state inheritance/handoff. | Gate note posted. |
| #507 | Explicitly excluded from autonomous work until authorization names #507. | Leave open. |
| #516 | HTTP/API credential proxy backend and upstream secret injection/redaction. | Gate note posted. |
| #518 | Credential leakage/bypass harness and environment/log/API probing. | Gate note posted. |

## External-State Blocked

| Issue | Blocker | Latest evidence |
| --- | --- | --- |
| #478 | GHCR public image pull proof missing. | `tests/release/test-ghcr-matrix.sh` passed; `scripts/verify-release-assets.sh v2026.6.28` still fails GHCR pull with `unauthorized`. |
| #488 | Apple `container` runtime not installed on mutsu. | Spike runner exists and can reach mutsu; latest transcript state is Defer. |
| #489 | Apple provider implementation depends on #488. | Dependency-refresh comment posted; keep blocked until #488 proves runtime contract. |

## Deferred / Dependency-Only

| Issue | Status |
| --- | --- |
| #114 | Alpine/Proxmox VM provisioning epic deferred until 2026-08-17. |
| #118 | Alpine agentic-dev profile deferred until 2026-08-17. |
| #119 | Backend abstraction remains deferred; Proxmox backend is still a stub, so not closable. |
| #120 | Deploy/lifecycle script updates deferred until 2026-08-17. |
| #438 | Apple host support parent remains open pending #488 and #489. |
| #503 | Security market-readiness epic remains open; #509 is closed but transport evidence, credential proxy posture, and GHCR verification remain unresolved. |
| #517 | Protocol credential adapters passed preflight but depend on #516, which is authorization-gated. |

## Recommended Next Moves

1. For implementation: get explicit authorization for a specific gated issue before touching credential, CA, provider-auth, SSH-key, PAT, or transport-secret surfaces.
2. For release evidence: make GHCR packages public or provide an authenticated verification path, then rerun `scripts/verify-release-assets.sh v2026.6.28` without `--skip-ghcr` for #478.
3. For Apple support: install Apple `container` on mutsu or another supported Apple Silicon macOS 26 host, rerun the #488 spike workflow, then decide whether #489 can start.
4. For deferred VM platform work: recheck #114/#118/#119/#120 on or after 2026-08-17.
5. For backlog hygiene: keep #503 open as the aggregate security-readiness tracker until the remaining evidence and authorization-gated items are resolved or explicitly accepted as limitations.
