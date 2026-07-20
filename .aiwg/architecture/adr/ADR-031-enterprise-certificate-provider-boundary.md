# ADR-031: Enterprise Certificate Providers Use an Out-of-Process Contract

## Status

Accepted (2026-07-18; operator-approved on `agentic-sandbox#411`, with a
CLI-first execution refinement)

## Context

The public runtime already has a shared `GrpcCaBackend` Rust trait, a local
embedded CA, a remote mock, and fail-closed selection for a future remote
provider. Issue 411 now requires:

- a pluggable contract that can support any conforming certificate manager;
- a private repository for enterprise provider implementations;
- a deployment model that can support enterprise builds, discoverable
  expansions, Cargo modules, or a better researched alternative.

The boundary handles CA credentials and signing authority. It must therefore
survive provider API drift and release skew without loading untrusted or
proprietary code into the management process.

## Decision

### 1. Public domain contract, private provider implementations

The public `agentic-sandbox` repository owns:

- the versioned protocol schema and compatibility policy;
- generated client types used by management;
- SPIFFE ID, CSR, returned-certificate, trust-domain, TTL, and chain
  validation;
- renewal scheduling, jitter, expiry gates, listener hot reload, and
  fail-closed policy;
- a conformance harness and a non-secret mock provider.

The private enterprise repository owns:

- OpenBao/Vault, step-ca, and SPIRE adapters;
- provider authentication and credential-source implementations;
- enterprise packaging manifests and provider-specific runbooks;
- provider integration tests that require private infrastructure.

This keeps security policy in the public core and provider mechanics behind
the enterprise boundary.

### 2. Primary extension mechanism: CLI-first provider executable

Enterprise adapters are executables with a versioned command contract.
Management invokes an explicitly configured absolute executable path on demand,
writes one bounded request to stdin, reads one bounded response from stdout,
and waits for the process to exit. No shell is involved. This is the baseline
transport and requires no continuously running provider service.

Adapters may also implement `serve` mode for high-throughput fleets. That mode
exposes the same logical protocol over a management-owned Unix socket
(`0700` directory, `0600` socket). A Windows named-pipe profile may be added
later. No TCP listener is enabled by default.

Executable discovery is explicit rather than PATH-scanned. Configuration names
an allowlisted absolute path and, for enterprise distributions, the expected
artifact digest. `GetProviderInfo`/`describe` is executed before readiness.

The protocol has these logical operations:

| Operation | Purpose |
| --- | --- |
| `GetProviderInfo` | Protocol version, implementation identity, build provenance, and capabilities |
| `GetTrustBundle` | Current bundle, trust domain, immutable revision, and validity metadata |
| `WatchTrustBundle` | Stream complete bundle updates and rotation events |
| `SignWorkloadCsr` | Sign a validated CSR for one SPIFFE ID and bounded TTL |
| `Health` | Readiness, degraded state, last successful issuance, and non-secret diagnostics |

Minimum request metadata includes a unique request ID, SPIFFE ID, CSR DER,
requested TTL, and expected trust domain. Minimum response metadata includes
the certificate chain, serial/fingerprint, not-before/not-after, bundle
revision, echoed request ID, and effective TTL. A provider audit ID is required
only when the adapter advertises `PROVIDER_AUDIT_ID`.

Every optional behavior is capability-negotiated. Initial capabilities are:

- `CSR_SIGNING`
- `TRUST_BUNDLE_WATCH`
- `REQUESTED_TTL`
- `PROVIDER_AUDIT_ID`
- `REVOCATION`
- `KEY_ATTESTATION`

Unknown capabilities are ignored. Missing required capabilities fail startup.
The core supports protocol major version 1 and rejects unknown major versions;
minor additions must be backward compatible.

CLI commands are:

```text
agentic-ca-provider describe
agentic-ca-provider trust-bundle
agentic-ca-provider sign
agentic-ca-provider health
agentic-ca-provider serve        # optional
```

Machine requests and responses use stdin/stdout only. Human diagnostics go to
stderr. Every adapter ships `--help`, shell completions where practical, and
detailed man pages describing configuration, opaque secret references,
capabilities, examples with synthetic data, and failure recovery. Documentation
must help humans and agents discover safe secret-reference workflows without
printing or accepting raw secret values.

### 3. Ownership of keys, policy, and renewal

- Agents generate leaf keys and send CSRs. Neither management nor the adapter
  receives leaf private keys.
- The provider owns CA-key custody and provider authentication.
- The core owns identity authorization and validates every returned leaf.
- The core schedules renewal at approximately 50% of lifetime plus bounded
  jitter, constrained by the returned validity window.
- The provider may shorten TTL but cannot expand it beyond core policy.
- Bundle rotation is delivered as complete snapshots; an empty, stale, or
  mismatched bundle fails closed.
- Existing TLS sessions remain established through rotation; new handshakes
  use the validated replacement material.

### 4. Secret contract

Provider credentials and CA key material are prohibited from:

- protocol request/response fields;
- environment variables and command-line arguments;
- logs, traces, metrics labels, crash reports, or tracked configuration.

Adapter configuration may contain only opaque credential references. Preferred
sources are workload identity, an HSM/PKCS#11 handle, an inherited file
descriptor, or an operating-system credential store. File fallback requires
an owner-only path and documented rotation.

### 5. Repository topology

The first-party enterprise adapters use one private monorepo:

- Primary: `roctinam/agentic-sandbox-enterprise-ca` on Gitea.
- Read-only mirror: `jmaly/agentic-sandbox-enterprise-ca` on GitHub.

Both repositories remain private. Gitea is authoritative. Mirroring is
one-way from Gitea to GitHub; direct pushes to the GitHub mirror are disabled
by policy because Gitea push mirroring force-updates the destination.

The private repository begins with:

```text
adapters/
  openbao/
  step-ca/
  spire/
crates/
  provider-runtime/
conformance/
packaging/
runbooks/
man/
```

One monorepo is preferred initially because the adapters share the protocol
runtime, conformance suite, documentation conventions, release manifest, and
mirror policy. An adapter moves to its own repository only when it has distinct
licensing, ownership/access control, release cadence, or supply-chain isolation
requirements. Third-party providers may always live independently.

Protocol schema and generated conformance fixtures are consumed from a tagged
public core release. Private dependencies are pinned by immutable commit and
lockfile or published to an authenticated alternate registry with
`publish = ["enterprise"]`. Registry credentials use Cargo credential
providers, not plain tracked configuration.

Repository creation is a separate consequential action and is not authorized
by this proposed ADR alone.

### 6. Distribution

The primary enterprise distribution is a signed bundle containing:

- the unmodified public management binary;
- one or more on-demand provider CLI binaries;
- a distribution manifest pinning every artifact by digest;
- non-secret configuration and optional service-manager units for `serve`
  mode;
- man pages and machine-readable command/schema documentation;
- SBOM, provenance, compatibility matrix, and operator runbook.

A special enterprise management build is allowed only as a compatibility
profile. Compile-time Cargo features may embed an adapter where a supervised
sidecar is impossible, but the adapter must implement the same domain
contract, be disabled by default, and undergo equivalent conformance tests.

Native Rust dynamic-library plugins are rejected. They do not provide process
or credential isolation and require an explicitly maintained ABI across
toolchains, allocators, ownership rules, and panic behavior.

## Consequences

### Positive

- Provider failures and credentials are isolated from management.
- Providers do not need to remain loaded between issuance or health commands.
- Detailed CLI help/man pages provide a common discovery surface for operators
  and agents.
- Provider adapters can be written in Rust, Go, or another language.
- Public users can implement the contract without proprietary source access.
- Provider releases and security patches do not require changing core policy.
- Enterprise licensing and distribution remain cleanly separated.
- SPIRE-style streaming updates map naturally to the contract.

### Negative

- Each on-demand operation pays process startup cost; high-throughput fleets
  may supervise optional `serve` mode.
- Protocol and artifact compatibility require a maintained matrix.
- Local IPC and adapter lifecycle add operational and test surface.
- Sidecar packaging is more work than one optional Cargo dependency.

### Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Adapter impersonation | Owner-only local socket, peer identity, signed artifact and provenance |
| Protocol downgrade | Major-version allowlist; reject unsupported required capabilities |
| Malicious/buggy certificate response | Core revalidates identity, chain, TTL, key match, and trust domain |
| Provider credential leakage | Adapter-local opaque references; no secret protocol fields; output redaction |
| Mirror divergence | One authoritative Gitea remote; one-way protected mirror |
| Adapter outage | Fail closed for issuance; retain valid active sessions only until leaf expiry |
| Supply-chain compromise | Immutable artifact digests, lockfiles, SBOM, provenance, signature verification |

## Alternatives Considered

1. **Private Cargo crates compiled into management** — retained only as a
   constrained fallback because it couples releases and shares process trust.
2. **Rust dynamic libraries** — rejected due to ABI and in-process safety
   concerns.
3. **Direct provider clients in public management** — rejected because it
   mixes editions, provider credentials, and API dependencies into core.
4. **SPIFFE Workload API only** — use directly for SPIRE deployments where it
   fits, but it does not by itself normalize OpenBao/step-ca CSR signing,
   provider health, and enterprise audit metadata.

## Migration

See
`@.aiwg/deployment/migration-plan-enterprise-ca-providers.md`.

## References

- @.aiwg/reports/impact-assessment-issue-411.md
- @.aiwg/architecture/adr/ADR-024-unified-spiffe-identity.md
- @.aiwg/architecture/adr/ADR-025-embedded-ca-and-issuance.md
- @.aiwg/architecture/adr/ADR-027-cert-lifecycle-and-hot-reload.md
- @docs/security/enterprise-ca-provider-contract.md
- <https://spiffe.io/docs/latest/spiffe-specs/spiffe_workload_api/>
- <https://openbao.org/api-docs/secret/pki/>
- <https://smallstep.com/docs/step-ca/provisioners/>
- <https://doc.rust-lang.org/reference/abi.html>
- <https://doc.rust-lang.org/cargo/reference/registry-authentication.html>
- <https://docs.gitea.com/usage/repository/repo-mirror>
