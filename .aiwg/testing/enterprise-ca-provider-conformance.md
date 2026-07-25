# Enterprise CA Provider Command Conformance — Issue 411

**Date:** 2026-07-20
**Protocol:** v1.0
**Status:** Public Phase E1 Passed

## Implemented Surface

- Versioned provider information and capability negotiation.
- On-demand `describe`, `health`, `trust-bundle`, and `sign` commands.
- Direct absolute-path invocation without shell or PATH discovery.
- Bounded JSON stdin/stdout and bounded, suppressed diagnostics.
- Command timeout and child termination.
- One-hour default leaf request for the remote backend.
- Trust-domain and request-ID correlation.
- Trust-bundle parsing and revision binding.
- Returned leaf CA-signature, SPIFFE URI-SAN, CSR public-key, current-validity,
  and maximum-TTL validation.
- Required provider audit ID when the capability is advertised.
- Public mock provider and detailed man-page source.
- Strict protocol-v1 JSON Schema and deterministic synthetic wire fixtures.
- Dependency-free fixture validation for private and third-party adapters.

## Verification

| Command | Result |
| --- | --- |
| `cargo test --manifest-path management/Cargo.toml --test grpc_ca_provider_command` | 8 passed |
| `cargo test --manifest-path management/Cargo.toml grpc_ca_provider_protocol --lib` | 5 passed |
| `python3 scripts/check-grpc-ca-provider-contract.py` | Passed |
| `python3 -m unittest tests/contracts/test_grpc_ca_provider_contract.py` | 5 passed |
| `cargo test --manifest-path management/Cargo.toml grpc_ca_backend --lib` | 2 passed |
| `cargo test --manifest-path management/Cargo.toml bootstrap_enrollment --lib` | 9 passed |
| `make test` | Passed across management, agent, and CLI crates |
| `cargo fmt --manifest-path management/Cargo.toml --check` | Passed |
| `python3 scripts/check-doc-links.py` | Passed |
| `git diff --check` | Passed |

The command integration suite covers:

1. provider discovery/help;
2. successful information, health, bundle fetch, and CSR signing;
3. relative executable rejection;
4. timeout and child termination;
5. malformed response rejection;
6. oversized response rejection;
7. degraded-health rejection;
8. suppression of synthetic secret-bearing stderr.

## Explicitly Unmet Issue 411 Gates

The public command boundary does not by itself complete fleet hardening:

- No OpenBao/step-ca/SPIRE adapter is included.
- The approved private first-party provider monorepo exists; production
  OpenBao/step-ca/SPIRE adapters have not been implemented.
- The shared lifecycle policy and running TLS agent renewal loop schedule
  renewal at 50–60% of leaf lifetime. Renewal is authenticated by the current
  mTLS identity and atomically replaces only a validated matching leaf.
- Production management server material uses the atomic rustls hot-reload
  resolver.
- The 30/7/1-day CA/intermediate states and lifetime-relative leaf renewal
  state are exported as Prometheus metrics; deployment-specific paging rules
  remain operator configuration.
- AC-6 passes for both management identity rotation and authenticated agent
  leaf renewal while a live control/PTY-shaped stream remains active.
- Provider executable digest/signature enforcement and optional `serve` mode
  remain later distribution work.

These items remain required before issue #411 can be declared resolved.
