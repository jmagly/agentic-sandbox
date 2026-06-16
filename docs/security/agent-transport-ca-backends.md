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
| `AGENTIC_GRPC_CA_AGENT_LEAF_TTL_SECS` | `86400` | Agent leaf lifetime. |
| `AGENTIC_GRPC_CA_SERVER_LEAF_TTL_SECS` | `604800` | Management mTLS server leaf lifetime. |
| `AGENTIC_GRPC_CA_RENEW_BEFORE_SECS` | `21600` | Renewal window before expiry. |

`AGENTIC_GRPC_CA_BACKEND=remote` is fail-closed until an
operator-approved remote provider adapter is implemented. Use
`remote-mock` only for provider-boundary integration tests and runbook
validation.

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
leaf renewal uses the same helper path, but the current listener reads server
material at startup, so rotate it with a bounded management restart.

## Bootstrap Enrollment

Bootstrap enrollment signs in-agent CSRs through the selected backend. The
request remains bound to a single-use token and the requested SPIFFE id. CSR
signing rejects:

- non-SPIFFE identities,
- CSR URI-SAN mismatch,
- subject common names,
- consumed or expired bootstrap tokens, and
- unavailable or unsupported CA backends.

## Reset And Recovery

For a workstation reset:

1. Stop provisioning new agents.
2. Back up or remove `<SECRETS_DIR>/grpc-local-ca`.
3. Remove stale per-agent leaves under `<SECRETS_DIR>/grpc-mtls`.
4. Restart management if the mTLS listener trusts the old CA.
5. Reprovision agents so they receive material from the new local CA.

For remote CA outage testing, set `AGENTIC_GRPC_CA_BACKEND=remote`; management
startup fails closed. Set `AGENTIC_GRPC_CA_BACKEND=remote-mock` to exercise the
remote provider boundary without external OpenBao, step-ca, or SPIRE services.
