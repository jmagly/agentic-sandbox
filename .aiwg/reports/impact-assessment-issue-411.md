# Impact Assessment — Issue 411 Enterprise CA Provider Boundary

**Date:** 2026-07-18
**Status:** Proposed
**Trigger:** Operator guidance on `agentic-sandbox#411`
**Related:** `agentic-sandbox#492`, `agentic-sandbox#493`, ADR-025, ADR-027

## Executive Summary

Issue 411 originally tracked fleet certificate issuance, renewal, and hot
reload. Its local and remote-boundary child tracks were delivered in commit
`df78783`. The remaining architecture decision is how to turn the current Rust
trait into an enterprise-capable extension boundary without coupling the public
runtime to proprietary CA implementations or exposing CA credentials.

The recommended change is additive:

1. Keep identity policy, renewal scheduling, validation, and hot reload in the
   public `agentic-sandbox` repository.
2. Define a versioned, provider-neutral CA protocol in the public repository.
3. Implement proprietary OpenBao, step-ca, and SPIRE adapters in one private
   first-party enterprise-provider monorepo.
4. Invoke adapters as CLI executables over bounded stdin/stdout by default, with
   optional local-socket `serve` mode for high-throughput fleets.
5. Ship an enterprise distribution that pins the core and adapter artifacts by
   immutable version and digest.

No existing private keys, credentials, or repositories were accessed while
producing this assessment.

## Current-State Evidence

- `management/src/grpc_ca_backend.rs` defines `GrpcCaBackend` and provides
  `local` and `remote-mock` implementations.
- The trait supports CSR signing and a static trust bundle but has no protocol
  version, capability negotiation, health state, audit metadata, trust-bundle
  update stream, or provider-owned credential reference.
- `AGENTIC_GRPC_CA_BACKEND=remote` intentionally fails closed.
- `docs/security/agent-transport-ca-backends.md` documents the local and mock
  behavior.
- ADR-027 requires one-hour fleet leaves, renewal near 50% of lifetime plus
  jitter, expiry monitoring, and live-session-safe reload.
- The rustls reload spike proves new handshakes can observe rotated material
  without dropping established connections.

## Impact Scope

| Area | Impact | Compatibility |
| --- | --- | --- |
| Management CA selection | Add a protocol-backed backend | Existing `local` remains default |
| Enrollment | Route CSR signing through provider client | SPIFFE ID and CSR validation stay in core |
| Trust bundles | Add versioned fetch/watch behavior | Static bundle remains supported |
| Renewal | Add capability-aware scheduling and provider health | Existing local behavior remains |
| Secrets | Move remote-provider credentials behind adapter boundary | No secret fields in public protocol |
| Packaging | Add enterprise adapter artifact and distribution manifest | Community build remains self-contained |
| Operations | Add adapter health, audit, rotation, and rollback runbooks | Remote selection continues to fail closed |

## Options Considered

Scores use 1 (poor) through 5 (strong). Security and isolation are weighted
twice because the extension handles CA credentials and signing authority.

| Option | Security / isolation | Compatibility | Extensibility | Operations | Supply chain | Weighted result |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| A. Compile private crates into an enterprise binary with Cargo features | 2 | 4 | 3 | 3 | 3 | 20/35 |
| B. Load Rust dynamic-library plugins in the management process | 1 | 2 | 4 | 2 | 2 | 15/35 |
| **C. Versioned CLI-first out-of-process provider protocol** | **5** | **5** | **5** | **5** | **4** | **33/35** |
| D. Make management directly implement each remote CA API | 2 | 3 | 2 | 2 | 3 | 17/35 |

### A. Compile-time private crates

Cargo features and optional dependencies are stable and straightforward.
However, every provider change requires rebuilding the management binary, the
private dependency enters the core process and dependency graph, and a
credential-handling defect shares the management server's full privileges.
This remains a supported fallback for constrained single-binary deployments,
not the primary enterprise extension model.

### B. Rust dynamic libraries

Dynamic loading looks discoverable but creates an unsafe in-process boundary.
Rust only provides a stable contract where an explicit ABI is defined; a
native Rust trait is not a durable plugin ABI. A C ABI or an ABI-stability
framework would add ownership, panic, allocator, and toolchain constraints
without isolating credentials or faults. Rejected as the default.

### C. CLI-first out-of-process protocol (recommended)

A command protocol gives explicit versioning, process and credential isolation,
independent release cadence, language neutrality, and clean health/audit
semantics without requiring a continuously running service. Management uses an
explicit absolute executable path and bounded stdin/stdout; an optional
owner-only Unix-socket mode amortizes startup cost at fleet scale. The cost is
process startup per baseline operation and a protocol compatibility matrix.

### D. Direct integrations in management

This minimizes moving pieces initially but mixes product editions, provider
authentication, API drift, and provider dependencies into the public runtime.
It also makes third-party implementations impractical. Rejected.

## Security and Trust-Boundary Findings

1. The core must validate requested SPIFFE IDs, CSR URI-SANs, returned
   certificate identity, chain, validity window, and trust domain. Provider
   success is not sufficient evidence.
2. Leaf private keys remain generated and retained by the agent. The provider
   receives a CSR, never a leaf private key.
3. CA/provider credentials are adapter-local. They must not appear in protocol
   messages, environment variables, command-line arguments, logs, or tracked
   configuration.
4. The adapter may refer to an HSM, workload identity, file descriptor, or
   operating-system credential-store item by opaque reference. It must not
   return CA private key material.
5. The core fails closed when the selected adapter is unavailable,
   incompatible, returns an invalid identity, or serves a stale/empty trust
   bundle.
6. Existing sessions may use their negotiated leaf until expiry; new issuance
   stops when provider health or validation fails.
7. The contract and generated bindings are public so third parties can
   implement the boundary without access to proprietary adapters.

## Research Basis

- The SPIFFE Workload API uses versioned gRPC services and streaming responses
  to distribute rotated SVIDs and trust bundles:
  <https://spiffe.io/docs/latest/spiffe-specs/spiffe_workload_api/>.
- SPIFFE trust bundles change over time and must retain their trust-domain
  association:
  <https://spiffe.io/docs/latest/spiffe-specs/spiffe_trust_domain_and_bundle/>.
- SPIRE's default X.509-SVID rotation strategy is one-half of lifetime:
  <https://spiffe.io/docs/latest/deploying/spire_agent/>.
- OpenBao exposes authenticated role-based CSR signing and warns that
  verbatim/issuer-override endpoints are privileged:
  <https://openbao.org/api-docs/secret/pki/>.
- Vault PKI roles can constrain requested URI SANs:
  <https://developer.hashicorp.com/vault/api-docs/secret/pki>.
- step-ca provisioners have different signing, renewal, and revocation
  capabilities, which argues for capability negotiation rather than
  provider-name conditionals:
  <https://smallstep.com/docs/step-ca/provisioners/>.
- Cargo features are conditional compilation, while alternate/private
  registries and credential providers are supported distribution mechanisms:
  <https://doc.rust-lang.org/cargo/reference/features.html>,
  <https://doc.rust-lang.org/cargo/reference/registries.html>,
  <https://doc.rust-lang.org/cargo/reference/registry-authentication.html>.
- Gitea supports one-way push mirroring and warns that it force-pushes the
  destination:
  <https://docs.gitea.com/usage/repository/repo-mirror>.

## Decision and Follow-Up

Adopt Option C; ADR-031 was operator-approved with the CLI-first refinement.
Preserve Option A only as a documented packaging fallback behind the same
domain-level contract.

Implementation is split into independently reviewable units:

1. Public protocol and conformance fixtures.
2. Protocol-backed management client with fail-closed validation.
3. Private adapter repository and OpenBao adapter.
4. step-ca and SPIRE adapters as demand requires.
5. Enterprise distribution, provenance, and operational runbooks.
