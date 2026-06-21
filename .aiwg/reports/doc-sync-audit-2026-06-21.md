# Doc Sync Audit — 2026-06-21

## Summary

- Direction: `code-to-docs`
- Scope: `management`, `agent-rs`, `cli`, `scripts`, `images/qemu`, `docs`, `.aiwg`
- Trigger: release gate `doc-sync-code-to-docs`
- Result: PASS

The changed code surface is the PTY terminal transport and formal session
integration path. Documentation was reconciled to reflect the implemented
behavior before release prep.

## Code Surface Audited

- `management/agentic-sandbox-executor/src/bindings/pty_ws.rs`
- `management/agentic-sandbox-executor/src/bindings/pty_bridge.rs`
- `management/src/agent_pty_bridge.rs`
- `management/src/dispatch/dispatcher.rs`
- `management/src/session/registry.rs`
- `management/src/ws/connection.rs`
- `management/src/http/operator_auth.rs`
- `management/src/http/server.rs`

## Documentation Surface Audited

- `docs/contracts/bindings/pty-ws/v1/spec.md`
- `docs/contracts/extensions/pty-extensions/v1/spec.md`
- `docs/ws-protocol.md`
- `docs/glossary.md`

## Findings

### DOC-DRIFT-001 — Binary PTY hot path

Status: fixed before report.

The code adds `pty-ws.v1.binary`, `PW1O` server output frames, and `PW1I`
client input frames. The `pty-ws/v1` binding spec now documents negotiation,
binary frame layout, JSON fallback behavior, and conformance requirements.

### DOC-DRIFT-002 — PTY attach authorization scopes

Status: fixed before report.

The code adds observe/control/admin PTY attach scopes and bearer-token upgrade
handling. The binding spec now documents `pty:observe`, `pty:control`, and
`pty:admin`, including observer write denial.

### DOC-DRIFT-003 — Deterministic closed lifecycle

Status: fixed before report.

The code propagates bridge EOF/start failure, last-member leave, and command
result completion into retained `Closed` frames. The PTY extension spec now
documents the close lifecycle, exit-code propagation, and replay retention.

### DOC-DRIFT-004 — Formal session registry integration

Status: fixed before report.

The code registers externally owned `pty-ws` sessions in the formal session
registry and routes formal input/resize/signal control paths back to the
owning agent command. Existing session architecture docs remain consistent with
that behavior; `ws-protocol.md` now describes the formal protocol as
role-based and capability-specific.

### DOC-DRIFT-005 — Legacy wildcard terminal fanout

Status: fixed before report.

The code rejects legacy `agent_id="*"` subscriptions unless
`AGENTIC_WS_ALLOW_WILDCARD_SUBSCRIBE=true` is set. `ws-protocol.md` now marks
the wildcard as deprecated, disabled by default, and only appropriate for
trusted legacy dashboards during migration.

### DOC-DRIFT-006 — Controller terminology

Status: fixed before report.

The implemented `pty-ws/v1` profile is single-controller plus observers, while
the extension permits larger controller capacity when advertised. The glossary
and PTY extension spec now use capability-specific wording and default
`max_controllers` to `1` for the reference profile.

## Validation

- `python3 scripts/check-doc-links.py --docs-root docs`
- `git diff --check`
- `cargo fmt --manifest-path management/Cargo.toml --check`
- `cargo fmt --manifest-path agent-rs/Cargo.toml --check`
- `cargo fmt --manifest-path cli/Cargo.toml --check`
- `cargo test --manifest-path management/Cargo.toml --lib`
- `cargo test --manifest-path agent-rs/Cargo.toml --lib`
- `cargo test --manifest-path cli/Cargo.toml --bins`

## Manual Review

No unresolved documentation drift was found for the changed release surface.
