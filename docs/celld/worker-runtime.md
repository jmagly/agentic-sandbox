# `worker-celld` constrained runtime

`worker-celld` is an optional Worker/Wasm runtime kind, not an alias for `agent-rs` and not a VM or container provider. It is unavailable until the Celld adapter is enabled and correctly pinned. Runtime discovery always reports the boundary.

## Supported surface

The v1 bundle contract permits fetch handlers, JavaScript RPC/service bindings, durable storage and alarms, inbound WebSockets, outbound HTTP fetch, Wasm modules, and static assets. Every bundle declares only what it uses, has resource ceilings, a trust domain, a content digest, and an optional signature. Unknown fields and unknown capabilities fail validation.

The runtime never promises command execution, PTY sessions, workspace or agentshare mounts, process spawning, raw TCP, SSH, Docker, QEMU, or the normal `agent-rs` lifecycle. Celld's currently inert Node/raw-socket compatibility stubs are treated as unsupported rather than successful features. A workload needing one of these capabilities must select `host`, `docker`, or `qemu` explicitly.

## Build and deploy

1. Bundle `worker.mjs`, its modules, and assets without symlinks or path traversal.
2. Calculate the SHA-256 digest and set it in `bundle.json`.
3. Run `sandboxctl celld bundle-validate --file bundle.json`.
4. Verify the optional Sigstore or Ed25519 identity against the fleet policy before upload.
5. Deploy by immutable digest. Keep the previous digest addressable until rollback expires.

The reference [`InstanceCell` worker](../../runtimes/celld/instance-cell/worker.mjs) demonstrates transactionally accepted commands, an operation ledger, generation fencing, alarms, and an unknown-outcome retry that reuses its original operation ID. JavaScript, constrained A2A, and Rust-Wasm examples live under `examples/celld/`; the A2A adapter uses only fetch plus durable key/value storage and deliberately does not emulate executor PTY or workspace functions.

## Go/no-go

The runtime remains experimental until the #754 qualification report shows all supported APIs pass, every unsupported API fails loudly, resource ceilings terminate or throttle the offending isolate, a pinned rollback succeeds, and a 24-hour soak meets the error and cost thresholds. Hostile mutually untrusted tenants are a no-go on the same Celld fleet.
