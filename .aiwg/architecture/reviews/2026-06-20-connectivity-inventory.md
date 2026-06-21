# Connectivity Path Inventory

Date: 2026-06-20

Issues: #534, #535, #536, #537, #538, #539

## Summary

This inventory normalizes the current connectivity surface before SSH gateway,
transport-posture, and session-bus cleanup work. The current implementation has
three mostly separate planes:

- agent control identity over gRPC with UDS, vsock, or mTLS transport evidence;
- terminal attach over `pty-ws/v1`, legacy agent-scoped WebSocket, and the
  management session registry;
- legacy direct-runtime SSH helpers and documentation that predate ADR-029's
  gateway-mediated SSH direction.

Direct SSH is not the planned managed-profile default. It is a dev or
break-glass bypass path unless and until it is routed through the gateway with
short-lived SSH certificate or lease semantics. `pty-ws` remains the
collaborative/replay/fanout terminal path and is not interchangeable with SSH.

## Disposition Table

| Path | Connectivity role | Disposition | Owner issue | Notes |
| --- | --- | --- | --- | --- |
| `management/src/grpc.rs` | Agent gRPC `Connect` stream, registration, command dispatch, transport identity extraction. | Keep and normalize | #539, #536 | Plain TCP without transport identity is rejected. Legacy `x-agent-secret` is tested as rejected/ignored; keep this as the control-plane source of authenticated transport evidence. |
| `management/src/main.rs` | Listener setup for TCP, UDS, vsock, and mTLS wrapping. | Keep and normalize | #539, #536 | `AGENTIC_ALLOW_PLAINTEXT_TCP` is an explicit unsafe/dev acknowledgement. The listener surface should feed the canonical transport posture vocabulary. |
| `management/src/registry.rs` | Connected-agent registry and transport summary. | Normalize | #539 | Now owns the canonical `AgentTransportPosture` vocabulary: `mtls`, `uds`, `vsock`, `bootstrap-pending`, `plaintext-dev`, and `unknown`. |
| `management/src/http/admin_v2.rs` | Admin instance inventory, provisioning, retired legacy secret endpoints. | Normalize | #539, #536, #535 | Instance output now consumes the registry posture model. Running unconnected instances surface `bootstrap-pending`; missing evidence stays `unknown`. QEMU network metadata still exposes `ssh_port: 22` and needs #535/#537 review before gateway SSH implementation. |
| `management/src/http/operator_auth.rs` | Operator/admin HTTP auth over bearer, mTLS, and UDS peer credentials. | Keep | #404, #539 | Separate from agent-plane transport identity. Ensure future gateway SSH admin endpoints use this policy boundary or a successor authorization layer. |
| `management/src/http/events.rs`, logs, audit modules | Event and audit visibility. | Needs follow-up issue | #537, #538, #540 | Current audit vocabulary does not yet distinguish gateway SSH cert lease issuance, runtime direct SSH key use, and provider SSH credentials. |
| `management/src/audit/secrets_rotation.rs` | Secret rotation helpers including SSH key material assumptions. | Deprecate/refactor | #537 | Existing persistent SSH key rotation must be explicitly scoped to dev/break-glass or replaced by gateway SSH certificate lease backend semantics. |
| `management/src/http/aiwg_proxy.rs` | Direct SSH command/file proxy into VMs. | Deprecate or gateway-wrap | #540 | Shells out to `ssh` using runtime key paths. This bypasses gateway policy/audit guarantees and should not remain a managed-profile default path. |
| `management/src/agent_pty_bridge.rs` | Bridge from `pty-ws/v1` sessions to agent gRPC PTY commands. | Keep and normalize | #538, #523, #524 | Useful adapter, but session close/result and replay ownership must be consolidated through the canonical session bus. |
| `management/src/session/*` | Management-side session registry, replay, redaction, transcript. | Normalize | #538, #523 | Stronger replay/redaction model than executor-local registry, but terminology and ownership drift remain. |
| `management/agentic-sandbox-executor/src/bindings/pty_ws.rs` | Public `pty-ws/v1` WebSocket binding and in-memory session registry. | Normalize | #538, #522, #523 | Keep as the client attach protocol, but reconcile registry, role, replay cursor, and sequence vocabulary with management session state. |
| `management/agentic-sandbox-executor/src/bindings/pty_bridge.rs` | Executor PTY bridge abstraction and legacy no-op broadcast behavior. | Normalize/deprecate compatibility | #538, #525 | The no-op legacy echo behavior is compatibility-only and should not be treated as the canonical terminal path. |
| `cli/src/cmd/session.rs` | CLI `attach` and `session attach` paths. | Normalize | #538 | Two-argument attach prefers `pty-ws.v1`; one-argument top-level attach remains legacy agent output. Terminology should be aligned around controller/observer/member. |
| `cli/src/cmd/agent.rs`, `cli/src/cmd/vm.rs` | CLI admin instance listing/get paths. | Normalize | #539 | CLI now reads v2 `items` responses and renders the canonical `transport` and `transport_posture` fields. |
| `agent-rs/src/main.rs` | Agent transport selection, bootstrap enrollment, PTY host. | Keep and isolate legacy | #536, #539 | `AGENT_TRANSPORT=auto` prefers secure transport material and legacy secret metadata is omitted. TCP mode remains unauthenticated and should be dev/migration only. |
| `docs/API.md`, `docs/ws-protocol.md`, `docs/contracts/bindings/pty-ws/v1/spec.md` | API and terminal protocol docs. | Normalize | #538, #535 | `pty-ws/v1` must be described as the replay/fanout terminal protocol; legacy WebSocket attach must be compatibility-only. |
| `README.md`, `docs/reliability-*`, `docs/LIFECYCLE.md`, release docs | Operator docs with direct SSH examples/readiness assumptions. | Normalize | #535 | Direct `ssh agent@<vm-ip>` examples must be removed or marked dev/break-glass and linked to ADR-029/rollout. |

## Required Explicit Notes

| Topic | Current state | Disposition |
| --- | --- | --- |
| Direct runtime SSH | Present in docs and in `management/src/http/aiwg_proxy.rs`; QEMU instance network output still exposes port 22. | Dev/break-glass only. Managed access should move to gateway-mediated SSH. Cleanup owners: #535, #537, #540. |
| Gateway-mediated SSH | Described by ADR-029 and the 2026-06-19 rollout plan; implementation not present yet. | Planned first-class access option, not fallback. Implementation should follow #531 and use short-lived SSH cert or lease semantics. |
| gRPC mTLS/UDS/vsock | Implemented in management and agent transport identity paths. | Keep as agent machine identity plane. Normalize display and posture through #539. |
| Legacy plaintext TCP | Explicitly unsafe/dev gated on management listener side; agent TCP mode carries no legacy secret metadata. | Isolate behind explicit config and migration/dev language. Cleanup owner: #536. |
| Legacy agent secret/TOFU | gRPC rejects legacy-only metadata and retired rotation endpoints return gone. Residual docs/plans still mention migration phases. | Remove or mark obsolete/migration-only. Cleanup owner: #536 with #412/#507. |
| `pty-ws` | Best current client terminal attach direction. | Keep, but consolidate registry, auth, role, replay, and sequence vocabulary before #523. Cleanup owner: #538. |
| Legacy WebSocket attach | Still documented and reachable for compatibility paths. | Compatibility-only; normal clients should use `pty-ws/v1`. Cleanup owner: #538 and #525. |
| `Unknown transport` derivation | Previously emitted directly from `AgentTransportKind::Unknown`. | Canonical model now reserves `unknown` for genuinely missing evidence; running unconnected instances use `bootstrap-pending`. Cleanup owner: #539. |

## Unknown Transport Derivation Points

- `management/src/registry.rs`: `AgentTransportKind::Unknown` remains the
  internal default for agents registered without transport evidence, but
  public API rendering now goes through `AgentTransportPosture::unknown()`.
- `management/src/http/admin_v2.rs`: instance decoration derives public
  posture from the connected registry entry, from running unconnected state
  (`bootstrap-pending`), or from missing evidence (`unknown`).
- CLI consumers in `cli/src/cmd/agent.rs` and `cli/src/cmd/vm.rs` now render
  the same `transport` and `transport_posture` strings returned by admin-v2.
- Dashboard consumers should continue to rely on admin-v2 instance fields; no
  independent transport vocabulary should be introduced in `management/ui`.

## Follow-Up Ownership

| Finding | Owner |
| --- | --- |
| Direct SSH docs and examples need gateway-policy language. | #535 |
| SSH key rotation and long-lived runtime SSH key semantics need disposition. | #537 |
| Direct SSH AIWG proxy bypass needs explicit cleanup. | #540 |
| Legacy `AGENT_SECRET`, `x-agent-secret`, TOFU, and plaintext TCP residues need removal/isolation. | #536 |
| Split PTY/session attach terminology needs a canonical table before #523. | #538 |
| Transport posture API vocabulary and tests need to stay server-owned. | #539 |

## Verification

- `cd management && cargo test transport_posture_vocabulary_is_canonical --lib`
- `cd management && cargo test registered_host_context_includes_transport_and_daemon_status --lib`
- `cd management && cargo test running_instance_without_agent_reports_bootstrap_pending_transport --lib`
- `cd management && cargo test stopped_instance_without_agent_reports_missing_transport_evidence --lib`
- `cd cli && cargo test renders_list_table_from_array_response --bin sandboxctl`

