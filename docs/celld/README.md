# Celld integration

Celld support is experimental, optional, and disabled by default. It has three independently qualified roles:

1. durable `InstanceCell` orchestration for QEMU, Docker, and evaluated host runtimes;
2. the constrained `worker-celld` Worker/Wasm runtime;
3. managed Celld fleets deployed on existing substrates.

Start with [architecture.md](architecture.md), then use [worker-runtime.md](worker-runtime.md), [fleet-operations.md](fleet-operations.md), [storage-qualification.md](storage-qualification.md), [security.md](security.md), and [observability-and-runbooks.md](observability-and-runbooks.md). The delivery sequence and Titan approval checkpoints are in [qualification-roadmap.md](qualification-roadmap.md). Versioned machine contracts are under `docs/contracts/celld/`; the qualification matrix is `tests/celld/qualification-plan.md`.

## Enable a POC

Keep the feature off until the private listener, credential file, and exact upstream pin are ready:

```text
AGENTIC_CELLD_ENABLED=true
AGENTIC_CELLD_ENDPOINT=https://celld.internal
AGENTIC_CELLD_AUTH_KEY_ID=2026-08-poc
AGENTIC_CELLD_AUTH_KEY_FILE=/run/credentials/agentic-celld-auth
AGENTIC_CELLD_EFFECT_LEDGER_PATH=/var/lib/agentic-sandbox/celld/effects.db
AGENTIC_CELLD_TLS_CA_FILE=/run/credentials/celld-control-ca.pem
AGENTIC_CELLD_TLS_CLIENT_IDENTITY_FILE=/run/credentials/management-celld-identity.pem
AGENTIC_CELLD_CALLBACK_MTLS_CN=celld-fleet-poc
AIWG_TLS_CERT=/run/credentials/management-server-cert.pem
AIWG_TLS_KEY=/run/credentials/management-server-key.pem
AIWG_TLS_CLIENT_CA=/run/credentials/celld-callback-ca.pem
AIWG_TLS_CLIENT_AUTH=required
AGENTIC_CELLD_VERSION=v0.2.1
AGENTIC_CELLD_COMMIT=ae8fac053d79f971bfcb996054bb43eb2f9b05da
AGENTIC_CELLD_PROTOCOL_VERSION=celld-internal-v1
```

The auth file and combined PEM client-identity file must be mode 0600 (or stricter); the HMAC key must contain at least 32 bytes. Every non-loopback Celld endpoint requires the private CA, client identity, and the exact CN expected on Celld callbacks. The callback CN is taken only from a certificate verified by the management mTLS listener (`AIWG_TLS_CLIENT_AUTH=required`), never from an HTTP header. The effect-ledger parent directory must already exist on durable local storage and be writable only by the management service account; management creates the database as mode 0600 and rejects an existing group/world-accessible database. Startup fails closed if any required resource cannot be opened. Run `sandboxctl celld status`; it reports endpoint and version metadata but never the secret, secret contents, identity contents, or ledger records. With the flag unset, existing QEMU, Docker, and host flows do not construct a Celld client or effect ledger.

For a key rotation, signing always uses the active key above. Configure the old verification key in both management and the Worker for a bounded overlap of at most 15 minutes:

```text
AGENTIC_CELLD_AUTH_PREVIOUS_KEY_ID=2026-08-old
AGENTIC_CELLD_AUTH_PREVIOUS_KEY_FILE=/run/credentials/agentic-celld-auth-old
AGENTIC_CELLD_AUTH_PREVIOUS_VALID_FROM=2026-08-15T15:00:00Z
AGENTIC_CELLD_AUTH_PREVIOUS_VALID_UNTIL=2026-08-15T15:15:00Z
```

The Worker uses the corresponding `CELL_AUTH_PREVIOUS_KEY_ID`, `CELL_AUTH_PREVIOUS_KEY`, `CELL_AUTH_PREVIOUS_VALID_FROM`, and `CELL_AUTH_PREVIOUS_VALID_UNTIL` secret bindings. Partial configuration, duplicate key IDs, weak keys, malformed timestamps, and overlap windows longer than 15 minutes fail closed. Remove the previous key bindings after the overlap expires.

## API and CLI

The authenticated management surface is under `/api/v2/celld`: status, cells, commands, reconciliation, bundle validation, fleet validation, object-store evidence preflight, read-only fleet diagnosis, and advisory upgrade planning. `sandboxctl celld diagnose --file OBSERVATIONS.json` classifies a strict observation envelope; fixture/local inputs cannot claim live qualification. Upgrade plans are explicitly non-mutating until the CLQ-10 controller exists. InstanceCell alarms call the internal `POST /api/v2/celld/effects` boundary with a fresh, generation-bound HMAC and the original idempotency key. Management durably claims that identity before provider dispatch; terminal replays return the original result, while unknown outcomes are looked up and never assigned a replacement effect identity. Callback responses include `provider_dispatch_count`, the durable number of times management granted the operation's provider-dispatch owner. It is expected to remain one across retries and restarts; it records a dispatch boundary crossing, not an unsupported claim that the external provider completed the effect. `sandboxctl celld --help` exposes the operator diagnostics and validations.
