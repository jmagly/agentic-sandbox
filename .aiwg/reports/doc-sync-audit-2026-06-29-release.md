# Doc Sync Audit — 2026-06-29 Release

**Direction:** code-to-docs
**Scope:** release-v2026.6.35 (`v2026.6.34..HEAD`)
**Audited areas:** management, agent-rs, cli, scripts, images/qemu, docs, .aiwg

## Summary

The release delta after `v2026.6.34` contains five commits:

- `dc224f3` — defer Darwin release artifacts from the current release matrix.
- `86b8115` — add the HTTP credential proxy backend.
- `a869177` — stabilize vsock CID ownership and pty cleanup.
- `7d07afc` — keep pty-ws bridge output live for VM sessions.
- `599d855` — reap stale pty-ws controller sockets.

The credential proxy and Darwin release-matrix changes already had matching
documentation in `docs/API.md`, `docs/security/credential-proxy.md`,
`docs/security/attack-surface.md`, `docs/releases/runbook.md`, and
`docs/releases/verification.md`.

## Findings

### DOC-DRIFT-20260629-001 — pty-ws heartbeat timeout contract was stale

Severity: high

`docs/contracts/bindings/pty-ws/v1/spec.md` described a `30s` Ping interval and
`10s` Pong timeout, but the executor now sends Pings every `30s`, waits `90s`
for Pong, and then exits the connection loop so the normal member/controller
cleanup runs.

Fix: updated the keepalive section, close-code note, and conformance checklist
to describe stale-client reaping and controller-role release.

### DOC-DRIFT-20260629-002 — reconnect example implied unsupported client-id grace reuse

Severity: medium

`docs/contracts/extensions/pty-extensions/v1/examples/reconnect-replay.md`
described a controller grace window and prior `client_id` reuse. The current
executor uses connection-local client IDs and relies on clean-close/error or
heartbeat reap to release the old controller slot.

Fix: updated the example to describe immediate cleanup on observed disconnect,
heartbeat cleanup for half-open sockets, and fresh connection-local client IDs
on reattach.

## Validation

Planned release gates after this sync:

- `cargo fmt --manifest-path management/Cargo.toml --check`
- `cargo fmt --manifest-path agent-rs/Cargo.toml --check`
- `cargo fmt --manifest-path cli/Cargo.toml --check`
- `cargo test --manifest-path management/Cargo.toml --lib`
- `cargo test --manifest-path agent-rs/Cargo.toml --lib`
- `cargo test --manifest-path cli/Cargo.toml --bins`
- release script shell syntax and pin lint gates from `.aiwg/release.config`

Manual review required: none.
