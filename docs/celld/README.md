# Celld integration

Celld support is experimental, optional, and disabled by default. It has three independently qualified roles:

1. durable `InstanceCell` orchestration for QEMU, Docker, and evaluated host runtimes;
2. the constrained `worker-celld` Worker/Wasm runtime;
3. managed Celld fleets deployed on existing substrates.

Start with [architecture.md](architecture.md), then use [worker-runtime.md](worker-runtime.md), [fleet-operations.md](fleet-operations.md), [security.md](security.md), and [observability-and-runbooks.md](observability-and-runbooks.md). Versioned machine contracts are under `docs/contracts/celld/`; the qualification matrix is `tests/celld/qualification-plan.md`.

## Enable a POC

Keep the feature off until the private listener, credential file, and exact upstream pin are ready:

```text
AGENTIC_CELLD_ENABLED=true
AGENTIC_CELLD_ENDPOINT=https://celld.internal
AGENTIC_CELLD_AUTH_KEY_ID=2026-08-poc
AGENTIC_CELLD_AUTH_KEY_FILE=/run/credentials/agentic-celld-auth
AGENTIC_CELLD_VERSION=v0.2.1
AGENTIC_CELLD_COMMIT=ae8fac053d79f971bfcb996054bb43eb2f9b05da
AGENTIC_CELLD_PROTOCOL_VERSION=celld-internal-v1
```

The auth file must be mode 0600 (or stricter) and at least 32 bytes. Run `sandboxctl celld status`; it reports endpoint and version metadata but never the secret or secret contents. With the flag unset, existing QEMU, Docker, and host flows do not construct a Celld client.

## API and CLI

The authenticated management surface is under `/api/v2/celld`: status, cells, commands, reconciliation, bundle validation, fleet validation, object-store evidence preflight, and rolling-upgrade planning. `sandboxctl celld --help` exposes the same diagnostics and validations.
