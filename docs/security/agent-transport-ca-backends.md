# Agent Transport CA Backends

Agent gRPC mTLS uses SPIFFE-shaped identities:

```text
spiffe://<trust-domain>/agent/<instance_id>
```

The management server selects one CA backend at startup. Selection is explicit
so workstation deployments do not need a fleet CA, and distributed deployments
do not silently fall back to local private material.

## Backend Selection

| Variable | Default | Purpose |
| --- | --- | --- |
| `AGENTIC_GRPC_CA_BACKEND` | `local` | `local`, `remote-mock`, or `remote`. |
| `AGENTIC_GRPC_CA_TRUST_DOMAIN` | `sandbox.agentic.local` | Trust domain used in agent SPIFFE URI-SANs. |
| `AGENTIC_GRPC_LOCAL_CA_TRUST_DOMAIN` | unset | Compatibility alias for the local trust domain. |
| `AGENTIC_GRPC_CA_AGENT_LEAF_TTL_SECS` | local: `86400`; remote: `3600` | Agent leaf lifetime. |
| `AGENTIC_GRPC_CA_SERVER_LEAF_TTL_SECS` | `604800` | Management mTLS server leaf lifetime. |
| `AGENTIC_GRPC_CA_RENEW_BEFORE_SECS` | `21600` | Renewal window before expiry. |
| `AGENTIC_GRPC_CA_PROVIDER_EXECUTABLE` | unset | Absolute CLI provider path required by `remote`. |
| `AGENTIC_GRPC_MTLS_RELOAD_INTERVAL_SECS` | `30` | Management leaf reload and lifecycle-metric interval. |
| `AGENTIC_GRPC_LOCAL_CA_KEY_STORE` | `filesystem` | Explicit local-root private-key store: `filesystem` or `macos-keychain`. |
| `AGENTIC_GRPC_LOCAL_CA_KEYCHAIN_SERVICE` | `io.aiwg.agentic-sandbox.grpc-local-ca` | Public macOS Keychain service identifier. |
| `AGENTIC_GRPC_LOCAL_CA_KEYCHAIN_ACCOUNT` | `root-key:<trust-domain>` | Public macOS Keychain account identifier. |

`AGENTIC_GRPC_CA_BACKEND=remote` fails closed unless an absolute provider
executable is configured, advertises protocol v1 and `CSR_SIGNING`, returns a
valid trust bundle for the configured domain, and passes response validation.
The executable is invoked directly with fixed subcommands; no shell or PATH
lookup is used. Use `remote-mock` only for in-process boundary tests. The
`grpc-ca-provider-mock` executable exercises the real command boundary for
conformance and runbook validation; it is not a production provider.

The accepted enterprise provider boundary, repository topology, and deployment
model are documented in
[Enterprise CA Provider Contract](enterprise-ca-provider-contract.md) and
ADR-031.

The exact protocol-v1 schema, fixtures, and portable conformance command are in
[`docs/contracts/ca-provider/v1/`](../contracts/ca-provider/v1/spec.md).

## Local Workstation Backend

The local backend stores its root CA in:

```text
<SECRETS_DIR>/grpc-local-ca/grpc-local-root-ca.pem
<SECRETS_DIR>/grpc-local-ca/grpc-local-root-ca-key.pem
```

The directory is chmod `0700`; root certificate and private key files are
chmod `0600`. Per-agent leaves are written by the provisioning helper under:

```text
<SECRETS_DIR>/grpc-mtls/<vm_name>/agent.pem
<SECRETS_DIR>/grpc-mtls/<vm_name>/agent-key.pem
```

The helper reuses an existing leaf only when:

- the certificate and key are both present,
- the certificate contains exactly the expected SPIFFE URI-SAN,
- the certificate is currently valid, and
- the certificate expires after the configured renewal window.

Otherwise it renews the leaf by issuing a new certificate and key. Active PTY
or control streams keep using the TLS session they already established; renewed
material is used on the next reconnect or reprovision. Management mTLS server
leaf renewal uses the same helper path.

TLS agents also schedule renewal from the current certificate at 50–60% of its
valid lifetime. `RenewCertificate` authenticates the existing mTLS peer,
derives the SPIFFE identity from that transport certificate, and accepts only a
CSR for the same identity. The agent retains its private key, validates the
returned identity and public key, and atomically replaces the leaf certificate.
The active control stream and its PTY sessions remain connected; subsequent
connections load the renewed material.

The provider-neutral lifecycle policy in `cert_lifecycle` schedules short-lived
leaf renewal at 50% of lifetime plus deterministic, identity-scoped jitter of
up to 10%. It also classifies CA and intermediate expiry at the 30-day,
7-day, and 1-day gates. Management exports
`agentic_certificate_seconds_until_expiry`,
`agentic_certificate_expiry_gate`, and `agentic_certificate_renewal_due` for
the configured gRPC server leaf and client CA.

The management gRPC listener reloads a changed server certificate and key as a
validated pair. A malformed or mismatched replacement is rejected while the
last valid identity remains active. Existing TLS and PTY/control sessions are
not renegotiated; new handshakes use the replacement identity.

### Explicit macOS Keychain root-key storage

On macOS, setting
`AGENTIC_GRPC_LOCAL_CA_KEY_STORE=macos-keychain` retains the public root
certificate at the configured local-CA directory but stores the matching root
private key as a non-synchronizing generic-password item in the login
Keychain. Security.framework receives the private bytes directly in-process;
the implementation does not invoke a shell, put key material in argv or the
environment, or create a temporary private-key file.

Selection is explicit. If a filesystem root already exists and the configured
Keychain item is absent, startup fails closed instead of migrating or falling
back. The inverse partial state also fails closed. A locked Keychain,
interaction-required access, and unavailable Keychain in SSH, CI, or launchd
contexts are startup errors; they never activate the filesystem backend.

The Keychain service/account are public identifiers. Private values must never
be printed with `security ... -w`, exported, or included in logs. The complete
operator procedure and witnessed rotation boundary are in the
[macOS Host Runtime and Local CA Keychain Runbook](../operations/macos-host-runtime-keychain.md).

## Bootstrap Enrollment

Bootstrap enrollment signs in-agent CSRs through the selected backend. The
request remains bound to a single-use token and the requested SPIFFE id. CSR
signing rejects:

- non-SPIFFE identities,
- CSR URI-SAN mismatch,
- subject common names,
- consumed or expired bootstrap tokens, and
- unavailable or unsupported CA backends.

The static-cert gRPC mTLS listener uses the configured
`AGENTIC_GRPC_MTLS_CLIENT_CA` root to verify bootstrap-issued client leaves.
After the TLS handshake, management extracts the SPIFFE URI-SAN from the
verified leaf and normalizes it as the transport peer identity. Registration is
accepted only when the agent metadata `x-agent-instance-id` matches the
`/agent/<instance_id>` component in that SPIFFE id.

For container and host bootstrap flows this means two endpoints must be
reachable from the agent runtime:

- the HTTP bootstrap enrollment URL used once to exchange the token and CSR for
  mTLS material, and
- the gRPC mTLS listener used for the long-lived agent control stream.

## Reset And Recovery

For a filesystem-backed workstation reset:

1. Stop provisioning new agents.
2. Back up or remove `<SECRETS_DIR>/grpc-local-ca`.
3. Remove stale per-agent leaves under `<SECRETS_DIR>/grpc-mtls`.
4. Restart management if the mTLS listener trusts the old CA.
5. Reprovision agents so they receive material from the new local CA.

For a Keychain-backed workstation, do not copy or silently migrate the private
key. Follow the witnessed reset/rotation ceremony in the macOS runbook.
Private-key export/import is intentionally unsupported; replacement uses a new
root and complete agent re-enrollment.

For remote CA outage testing, set `AGENTIC_GRPC_CA_BACKEND=remote`; management
startup fails closed. Set `AGENTIC_GRPC_CA_BACKEND=remote-mock` to exercise the
remote provider boundary without external OpenBao, step-ca, or SPIRE services.
