# Agentic Sandbox June 2026 Rollup Announcement

**Status:** Draft for review
**Scope:** June 2026 month-to-date, `v2026.6.0` through `v2026.6.25`
**Prepared:** 2026-06-19

## Short Announcement

June was the month Agentic Sandbox moved from a VM-management prototype toward
a releaseable runtime substrate for persistent AI agents. The `2026.6.x` line
completed the Rust-native E2E migration, formalized host, Docker, and VM
runtime targets, retired shared-secret agent authentication from the secure
path, and built out a packaged release pipeline with native Linux packages,
public container images, checksum verification, and Apple Silicon host-direct
build coverage.

The headline change is that agent execution is now treated as a substrate
choice rather than a single VM path. Operators can run agents in VMs,
containers, or directly on the host, with each mode carrying explicit isolation
tradeoffs. Terminal sessions now route through the `pty-ws/v1` and
`AgentPtyBridge` surfaces, including native PTY and managed `tmux`, `screen`,
and `zellij` backends for cloneable operator sessions.

On the security side, June replaced the legacy shared-secret and TOFU defaults
with transport-bound identity work: UDS, vsock, gRPC mTLS, local CA material,
bootstrap tokens, CSR enrollment, SPIFFE agent identities, and fail-closed
checks for provisioning paths that lack secure transport material. The latest
release-prep work also adds static-cert mTLS regression coverage and clearer
operator diagnostics for bootstrap-enrolled agents.

The release pipeline matured at the same time. The project now has CalVer
release gates, doc-sync checks, native `.deb` and `.rpm` packaging, a
checksum-verifying installer, GHCR runtime image publication, SBOM/signing
hardening, GitHub release mirroring, and release asset verification hooks.
Several June tags were intentionally superseded as CI and publication edge
cases were discovered; the rollup below calls those out so operators can
distinguish canonical releases from release-attempt tags.

## Highlights

- **Rust E2E migration completed:** the old pytest E2E harness is retired in
  favor of Rust-native VM-backed suites, live-agent conformance, and
  restart-durability coverage.
- **Runtime substrate expanded:** host, Docker, and VM runtimes are now first
  class enough to be selected and reasoned about per instance.
- **Host runtime landed:** `agentic-host-runtime-daemon`, local host
  supervisor routing, host isolation-tier metadata, and managed host sessions
  give AIWG a bare-host execution target.
- **Terminal control matured:** `pty-ws/v1`, native/direct PTY, managed
  `tmux`, `screen`, and `zellij` backends, replay/keyframe behavior, and
  attach-path conformance moved terminal work from ad hoc streaming toward a
  session-host contract.
- **Secure transport became the default direction:** secure provisioning now
  centers on UDS, vsock, gRPC mTLS, local CA material, bootstrap enrollment,
  SPIFFE identity, and fail-closed rejection of retired shared-secret paths.
- **Release packaging became operator-facing:** native Linux packages,
  checksum-verifying installer support, public GHCR image publication, and
  Apple Silicon host-direct tarballs joined the release matrix.
- **Release operations hardened:** CI lanes gained bounded SSH behavior,
  deterministic tests, SHA/checksum mirroring, SBOM/signing upload hardening,
  and explicit publication verification.
- **Launch claims were qualified:** new security posture, credential posture,
  attack-surface, and doc-sync reports separate implemented controls from open
  hardening work.

## Canonical Release Trail

| Version | Date | Use For |
| --- | --- | --- |
| `v2026.6.0` | 2026-06-11 | Rust E2E migration, AGPL-3.0-only licensing, CI self-healing, conformance tiers. |
| `v2026.6.1` | 2026-06-14 | Host runtime substrate, direct and managed sessions, secure transport groundwork. |
| `v2026.6.2` | 2026-06-14 | Packaged release pipeline: native Linux packages, installer, GHCR matrix, Apple Silicon host-direct path. |
| `v2026.6.7` | 2026-06-16 | Local-first gRPC mTLS CA backend lifecycle and direct-delivery CalVer release config. |
| `v2026.6.12` | 2026-06-16 | Hardened Apple Silicon release lane plus CA/backend and PTY portability fixes. |
| `v2026.6.16` | 2026-06-17 | GitHub checksum mirror and host-runtime instance listing fix. |
| `v2026.6.19` | 2026-06-19 | Host runtime bootstrap enrollment and daemon config fix. |
| `v2026.6.24` | 2026-06-19 | Docker/VM bootstrap envelope plus provider helper packaging for managed Claude sessions. |
| `v2026.6.25` | 2026-06-19 | Static-cert gRPC mTLS regression coverage, bootstrap peer identity proof, and launch-review docs. |

## Superseded Release Attempts

Several June tags intentionally remain in history as release-attempt evidence:

- `v2026.6.3` through `v2026.6.6` iterated on GHCR namespace/latest behavior
  and bounded mutsu SSH behavior for the packaged release pipeline.
- `v2026.6.8` through `v2026.6.11` tightened mutsu Rust tool discovery,
  Darwin PTY `ioctl` portability, and deterministic local CA renewal tests.
- `v2026.6.13` through `v2026.6.15` hardened Apple Silicon smoke behavior,
  SBOM/signature upload, GitHub mirroring, and workspace-local release tools.
- `v2026.6.17` and `v2026.6.18` were superseded by `v2026.6.19` for the host
  bootstrap enrollment fix.
- `v2026.6.20` through `v2026.6.23` were superseded by `v2026.6.24` while the
  Docker/VM bootstrap envelope and release verification path were stabilized.

## Operator Notes

- Treat the June line as a fast stabilization series. Prefer the latest
  canonical tag for each capability area rather than the first tag that
  introduced it.
- Host runtime is explicit full-host execution. It is useful for local
  autonomy and cockpit workflows, but it is not a VM isolation boundary.
- Secure transport is the supported direction. New deployments should use
  bootstrap enrollment, mTLS, UDS, or vsock identity rather than retired
  `AGENT_SECRET` or `x-agent-secret` paths.
- Public release claims should stay scoped: local-first management, qualified
  runtime isolation, secure agent transport direction, and release packaging
  evidence. Do not claim hardened remote dashboard exposure, complete image
  digest pinning, complete UI hardening, or full supply-chain closure until the
  open launch evidence is complete.

## Verification Themes

The June release evidence repeatedly exercised:

- `cargo fmt` and Rust unit/integration suites across management, agent, and
  CLI crates.
- VM-backed Rust E2E tests, resource-stress tests, and host-runtime tests.
- Container entrypoint checks for accepting bootstrap enrollment and rejecting
  missing secure transport material.
- Package and installer smoke tests for Debian/RPM-family install paths.
- GHCR matrix linting, release asset verification, SBOM/signature upload
  checks, checksum mirroring, and doc link validation.
- Live host and container smoke proofs for mTLS bootstrap enrollment,
  registration, managed `tmux` sessions, and provider helper readiness.

## Suggested Social Copy

Agentic Sandbox's June 2026 line is a major substrate milestone: Rust-native
E2E, host/Docker/VM runtime selection, secure transport identity, managed
terminal sessions, native Linux packages, public runtime images, and a hardened
CalVer release flow. The work is still being reviewed, but the direction is
now clear: local-first persistent AI agents with explicit isolation tradeoffs,
transport-bound identity, and verifiable release artifacts.

## Source Releases

- [`v2026.6.0`](v2026.6.0.md)
- [`v2026.6.1`](v2026.6.1.md)
- [`v2026.6.2`](v2026.6.2.md)
- [`v2026.6.7`](v2026.6.7.md)
- [`v2026.6.12`](v2026.6.12.md)
- [`v2026.6.16`](v2026.6.16.md)
- [`v2026.6.19`](v2026.6.19.md)
- [`v2026.6.24`](v2026.6.24.md)
- [`v2026.6.25`](v2026.6.25.md)
