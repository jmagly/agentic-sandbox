# Enterprise CA Provider Contract

**Status:** Accepted architecture under ADR-031 for issue #411. Public protocol
v1 and the command-backed management boundary are implemented. No production
enterprise provider adapter is included yet; selecting `remote` without an
explicit conforming executable fails closed.

Agentic Sandbox keeps certificate identity policy in the public management
runtime while allowing enterprise CA integrations to ship independently.
Provider adapters are CLI executables invoked on demand through bounded
stdin/stdout. An optional owner-only local-socket mode supports high-throughput
fleets. Adapters sign validated CSRs and publish trust bundles; they do not
decide which workload identities are authorized.

## Responsibility Split

| Public management core | Provider adapter |
| --- | --- |
| Authorize the SPIFFE ID | Authenticate to the CA service |
| Validate CSR and URI-SAN | Translate to provider-specific issuance API |
| Enforce maximum TTL and renewal policy | Apply provider role/template |
| Validate returned leaf, chain, key match, and trust domain | Return certificate chain and audit identifier |
| Schedule renewal with jitter | Report supported capabilities and health |
| Reload validated material | Publish trust-bundle revisions |
| Emit non-secret metrics and audit events | Hold opaque provider credential references |

Agents generate their own leaf key pairs. Only the CSR crosses the provider
boundary.

## Logical Protocol

Protocol major version 1 defines:

```text
GetProviderInfo() -> {
  protocol_version,
  implementation,
  build_provenance,
  capabilities[]
}

GetTrustBundle(expected_trust_domain) -> {
  trust_domain,
  bundle_der[],
  revision,
  observed_at
}

WatchTrustBundle(expected_trust_domain) -> stream<TrustBundleSnapshot>

SignWorkloadCsr({
  request_id,
  spiffe_id,
  csr_der,
  requested_ttl_seconds,
  expected_trust_domain
}) -> {
  certificate_chain_der[],
  serial_or_fingerprint,
  not_before,
  not_after,
  bundle_revision,
  provider_audit_id?
}

Health() -> {
  state,
  last_successful_issuance,
  diagnostics_code
}
```

The Rust protocol types live in
`management/src/grpc_ca_provider_protocol.rs`. Requests and responses are
bounded JSON with explicit correlation identifiers. Unknown minor-version
fields and optional capabilities are ignored. An unknown major version, a
missing required capability, or an invalid response prevents the remote
backend from becoming ready.

## CLI Discovery and Invocation

Every adapter implements:

```text
describe       protocol, implementation, provenance, capabilities
trust-bundle   current complete trust bundle snapshot
sign           sign one validated CSR
health         readiness and non-secret diagnostics
serve          optional persistent local-socket transport
```

Management invokes an allowlisted absolute executable path directly, never
through a shell or ambient PATH discovery. One request is written to stdin and
one response is read from stdout with strict size and time bounds. Diagnostics
use stderr and must not contain secrets.

Provider packages include detailed man pages, `--help`, and machine-readable
schema/help output. Examples use synthetic credential references. The
documentation teaches operators and agents to discover and use opaque
secret-store/HSM/workload-identity references without exposing the underlying
secret.

The reference manual source is
`docs/man/grpc-ca-provider.1`. The public `grpc-ca-provider-mock` binary
implements the baseline commands for conformance testing only.

Public conformance evidence and the remaining fleet-hardening gaps are tracked
in `.aiwg/testing/enterprise-ca-provider-conformance.md`.

## Validation Rules

Management accepts an issuance response only when:

1. The leaf public key matches the CSR public key.
2. The leaf contains exactly the authorized SPIFFE URI-SAN and no prohibited
   subject identity.
3. The SPIFFE trust domain matches configuration.
4. The chain validates against the selected bundle revision.
5. The validity window is current and no longer than core policy permits.
6. The echoed request ID is present and matches.
7. A bounded provider audit ID is present when the adapter advertises the
   `PROVIDER_AUDIT_ID` capability.

Provider errors never trigger a silent local-CA fallback. Existing sessions can
continue only until their already-negotiated certificate expires.

## Secret Handling

The protocol has no field for a CA private key, provider token, enrollment
secret, passphrase, or HSM PIN. These values must not be placed in environment
variables, command-line arguments, logs, metrics, traces, crash reports, or
tracked files.

An adapter may resolve an opaque credential reference through:

- workload identity;
- HSM or PKCS#11 handle;
- inherited file descriptor;
- operating-system credential store;
- owner-only file as a documented fallback.

The public core treats the reference as configuration metadata and does not
resolve or print its secret value.

## Packaging

The recommended enterprise package includes the unchanged public management
binary, provider CLI binaries, man pages, optional `serve` service definitions,
and a signed manifest pinning each artifact digest. Native Rust dynamic-library
loading is not part of the contract.

Compile-time Cargo integration is an allowed constrained profile, not the
default. It must implement the same logical contract and pass the same
conformance fixtures.

## Repository Plan

First-party enterprise providers share one private, Gitea-hosted monorepo:

- `roctinam/agentic-sandbox-enterprise-ca` on Gitea.

The enterprise source is not mirrored to public forge providers. Repository
access and backups remain within the private Gitea administration boundary.

The shared repository keeps protocol runtime code, conformance tests, man-page
conventions, and release provenance consistent across providers. Split an
adapter only for distinct licensing, access control, ownership, release cadence,
or supply-chain isolation. Third-party adapters may live in independent
repositories from the start.

## Standards and Provider Mapping

- SPIFFE Workload API streaming semantics guide trust-bundle and rotation
  updates:
  <https://spiffe.io/docs/latest/spiffe-specs/spiffe_workload_api/>.
- OpenBao adapters should use role-constrained CSR signing and avoid privileged
  verbatim/issuer override endpoints:
  <https://openbao.org/api-docs/secret/pki/>.
- step-ca adapters must advertise the actual provisioner capabilities:
  <https://smallstep.com/docs/step-ca/provisioners/>.

See ADR-031 and the migration plan for the option analysis, rollout gates, and
rollback procedures.
