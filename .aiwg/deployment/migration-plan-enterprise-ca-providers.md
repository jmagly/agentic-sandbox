# Migration and Rollback Plan — Enterprise CA Providers

**Date:** 2026-07-18
**Status:** Proposed
**Decision:** ADR-031
**Issue:** `agentic-sandbox#411`

> Sequencing is expressed as scope units and gates, not calendar estimates.

## Migration Principles

- Add the provider path without changing the default `local` backend.
- Keep `remote` fail-closed until a provider passes conformance and staging
  gates.
- Generate leaf keys at the agent and send only CSRs.
- Do not silently fall back from a configured remote provider to the local CA.
- Make every cutover reversible without changing SPIFFE identity semantics.
- Never migrate or print CA private key material as part of automation.

## Phase E0 — Approve Contract and Repository Plan

Scope:

1. Review and accept or revise ADR-031.
2. Confirm the approved first-party provider monorepo name, owners, and one-way
   mirror direction.
3. Assign the protocol namespace and versioning owner.
4. Approve the first provider. OpenBao is recommended because it matches the
   operator's earlier selection and exposes role-constrained CSR signing.

Exit gate:

- ADR status is Accepted. (Complete 2026-07-18.)
- Repository ownership and private visibility are confirmed.
- No open security-review blockers.

Rollback:

- Reject or revise ADR-031. Runtime behavior remains unchanged.

## Phase E1 — Public Protocol and Conformance Harness

**Status:** Complete in the issue #411 working tree on 2026-07-20; verification
is recorded in
`@.aiwg/testing/enterprise-ca-provider-conformance.md`.

Scope:

1. Add the versioned CLI provider schema and Rust client.
2. Specify capability negotiation, error taxonomy, and compatibility rules.
3. Extend `remote-mock` to implement `describe`, `trust-bundle`, `sign`, and
   `health` over bounded stdin/stdout.
4. Add conformance fixtures for valid issuance, URI-SAN mismatch, wrong trust
   domain, TTL expansion, stale bundle, empty bundle, provider outage, and
   unsupported protocol/capability.
5. Add secret-absence assertions for messages, logs, and diagnostics.

Exit gate:

- Protocol v1 fixtures pass.
- Existing local backend tests remain green.
- `remote` fails closed without an explicit conforming provider executable.

Rollback:

- Disable or remove the protocol-backed selector. Local and existing mock
  implementations remain unchanged.

## Phase E2 — Protocol-Backed Management Client

Scope:

1. Add a `CommandGrpcCaBackend` adapter behind the existing domain trait.
2. Invoke an allowlisted absolute executable directly without a shell.
3. Validate provider version and required capabilities at startup.
4. Revalidate every returned leaf and trust bundle in core.
5. Add provider health, issuance outcome, renewal-failure, and bundle-revision
   metrics without secret-bearing labels.
6. Poll bundle revision on the renewal cadence and integrate validated updates
   with the rustls reload path; add optional socket streaming only with
   `serve` capability.

Exit gate:

- Mock conformance suite passes through the real process/stdin/stdout boundary.
- Invalid responses fail closed.
- Existing PTY/control sessions survive certificate rotation.
- New handshakes observe renewed server material.

Rollback:

- Restore the previous management binary/configuration while leaving current
  valid leaves in place until their bounded expiry.

## Phase E3 — Private Repository and OpenBao Adapter

Scope:

1. Create the approved private first-party provider monorepo and private GitHub
   mirror.
2. Protect the Gitea default branch; make Gitea authoritative.
3. Configure one-way mirroring and prohibit direct mirror writes.
4. Implement OpenBao role-based CSR signing using `/pki/sign/:role`, not
   privileged verbatim or issuer-override endpoints.
5. Authenticate with an approved workload identity, HSM-backed mechanism, or
   opaque credential reference.
6. Add integration tests for URI-SAN policy, TTL caps, auth failure, CA outage,
   bundle rotation, and audit identifiers.
7. Produce signed artifacts, SBOM, provenance, and checksums.

Exit gate:

- Adapter passes the public conformance suite.
- No CA/provider credential appears in environment, CLI, logs, artifacts, or
  repository history.
- Operator validates issuance and rotation in a non-production OpenBao mount.

Rollback:

- Stop the adapter and return management to `remote-mock` in staging or
  `local` for workstation deployments. Fleet production remains fail-closed;
  it does not silently use the local CA.

## Phase E4 — Enterprise Distribution

Scope:

1. Publish a signed distribution manifest that pins core and provider CLI
   digests.
2. Ship detailed man pages, `--help`, and machine-readable schema/help output.
3. Add optional systemd/service-manager units for high-throughput `serve` mode.
4. Add compatibility, upgrade, recovery, key-rotation, and audit runbooks.
5. Add 30/7/1-day gates for long-lived CA/intermediate material and
   lifetime-relative gates for one-hour leaves.
6. Run live-PTY renewal acceptance coverage.

Exit gate:

- Clean-install, upgrade, downgrade, and rollback exercises pass.
- Provider outage remains fail-closed after the current leaf expires.
- Rotation audit records contain identifiers and timestamps, never keys.
- Distribution provenance and signatures verify offline.

Rollback:

1. Stop new provisioning.
2. Keep established sessions only while their current leaf remains valid.
3. Restore the previous signed core/adapter pair from the distribution
   manifest.
4. Restore the previous public trust bundle only if the corresponding issuer
   remains authorized and uncompromised.
5. Reissue leaves through the restored provider.
6. If CA compromise is suspected, revoke/retire the issuer and execute a
   witnessed key ceremony; do not restore compromised key material.

## Phase E5 — Additional Providers

Implement step-ca and SPIRE adapters only after the OpenBao path and protocol
have production evidence.

- step-ca capability mapping must reflect the selected provisioner's actual
  signing/renewal/revocation support.
- SPIRE should consume the standard Workload API streaming model rather than
  wrapping it in a lossy polling abstraction.
- Provider-specific behavior must not leak into core identity policy.

## Verification Matrix

| Requirement | Evidence |
| --- | --- |
| Any conforming implementation | Public protocol + independent conformance harness |
| Same SPIFFE identity | Core validation tests across local, mock, and provider |
| One-hour leaves | Returned validity and policy-cap tests |
| 50% renewal plus jitter | Deterministic scheduler property tests |
| Hot reload | Existing rustls spike promoted to production integration test |
| Active PTY survives | AC-6 end-to-end test |
| Expiry gates | Metrics/alert fixture tests |
| No silent fallback | Startup/outage tests |
| Secret hygiene | Fake-secret sentinel and log/artifact scans |
| Repository integrity | Protected primary, one-way mirror, signed immutable releases |
