# Agentic Sandbox Conformance Testing Protocol

This protocol defines how agentic-sandbox should interpret the standalone
`roctinam/agentic-sandbox-conformance` harness and how to graduate the current
skip set into actionable test tiers.

The conformance harness remains the publishable third-party contract test. The
agentic-sandbox repo owns the implementation-specific test protocol that
prepares a suitable system under test, decides which skipped scenarios are
acceptable for a release, and defines the integration suites needed for cases
the stub cannot drive.

## Current Baseline

Baseline reviewed: `77f8f5d fix(conformance): drive remaining failures to
zero`.

Recent fixes made the conformance-mode path useful as a CI gate:

- Pre-registers a deterministic conformance instance under
  `AIWG_CONFORMANCE_MODE=1`.
- Provides per-instance AgentCard and JWKS paths for JWS verification.
- Aligns runtime extension params with the published schemas.
- Adds A2A-compatible path aliases.
- Cleans handler response shapes for `messages:send`, `tasks/list`, and
  TaskStore metadata.
- Runs the harness from `.gitea/workflows/conformance.yml` against an ephemeral
  local management server.

Conformance-mode uses a stub dispatcher. That is intentional: it lets CI verify
wire contracts without provisioning a VM or container. It also means that any
test requiring a real task lifecycle, PTY frames, restart, or policy-specific
configuration must be handled by another tier.

## Testing Tiers

| Tier | Name | Backing SUT | Purpose | Merge behavior |
|---|---|---|---|---|
| T0 | Unit/contract | Rust unit tests and schema/lint jobs | Fast validation of handlers, stores, extension modules, OpenAPI coverage, and formatting. | Required on every PR. |
| T1 | Stub conformance | `AIWG_CONFORMANCE_MODE=1` management server plus `AcceptingMessageDispatch` | Publishable A2A wire-shape regression gate without a live agent loop. | Required on every PR; skips allowed only if categorized below. |
| T2 | Configured conformance | Same as T1, but with explicit auth/quota/legacy knobs enabled | Convert configuration-dependent skips into pass/fail assertions. | Required before v2.0 release candidates once knobs exist. |
| T3 | Live agent integration | Management server plus Docker or VM agent runtime | Drive task lifecycle, adapter-command execution, HITL, and real terminal states. | Required for orchestrator/substrate release gates. |
| T4 | PTY/session integration | Live runtime with pre-seeded PTY session and replay frames | Validate `pty-ws/v1` join, A2A-over-WS, role assignment, and replay semantics. | Required before promoting PTY binding from beta to stable. |
| T5 | Durability/restart | Management server with persistent SQLite state and controlled restart hook | Verify restart durability for idempotency and task state. | Required for release durability claims. |

## Skip Taxonomy

The T1 conformance job may skip a test only when the skip belongs to one of the
approved buckets below and the report includes the reason.

| Bucket | Count at baseline | Examples | Required owner action |
|---|---:|---|---|
| A: real agent loop required | 8 | terminal completed/failed/canceled/rejected shapes; HITL input-required and response validation | Cover in T3 with a controllable domain-specific agent. |
| B: spec deferred | 5 | runtime/v1 required enforcement; v2.2 multi-tenant semantics; runtime task metadata injection | Track against the target release and remove the skip when the feature ships. |
| C: pre-seeded PTY session required | 3 | A2A-over-WS round trip, join role assignment, replay keyframe | Cover in T4 with seeded sessions and captured frames. |
| D: configuration-dependent | 3 | missing auth returns 401; quota returns 429; forced 5xx inspection | Convert in T2 by adding harness-visible config knobs. |
| E: restart hook required | 1 | idempotency restart durability | Cover in T5; keep unit coverage for store-level behavior. |
| F: legacy artifact | 1 | deprecated v1 path emits Sunset header | Decide whether to implement Sunset compatibility or update the spec/harness to remove the legacy expectation. |

## Immediate Pickup Plan

The cheap subset is D + F. It should be handled before building the live-agent
suite because these are workflow/configuration questions, not runtime-agent
questions.

1. Add conformance-mode configuration flags for auth-required mode and a small
   quota limit.
2. Teach the workflow to run a second T2 harness pass with those flags enabled.
3. Decide the legacy v1 Sunset policy. If v1 compatibility remains, implement
   the `Sunset` header in the compatibility route. If v1 is removed, update the
   conformance harness expectation and release notes.
4. Keep T1 as the default fast gate; keep T2 separate so configuration failures
   are obvious.

## Live Agent Protocol

T3 needs a real agent loop, not a more complex stub. The test agent should be
small and deterministic:

- Accept one message and transition to `working`, then `completed`.
- Accept one message and produce a controlled infrastructure or domain failure
  with `fail_kind`.
- Accept one message and transition to `input-required` with a
  `hitl-prompt/v1` envelope.
- Accept a HITL response and either continue or reject invalid response payloads
  with `422`.
- Accept a cancelable long-running task and transition to `canceled` with
  `terminal_at`.
- Accept an `adapter-command/v1` metadata envelope and execute only the
  supported bounded command form.

The local deterministic T3 entrypoint is
`scripts/test-live-agent-conformance.sh`. It runs the executor crate's
synthetic live-agent tests, writes a markdown report plus redacted log, and uses
only synthetic fixtures for redaction coverage. It must not read live
credentials or environment secrets; any future live-runtime expansion that needs
real credential access requires a separate operator approval.

The live suite should record:

- management server commit and binary path;
- runtime kind and image or VM loadout;
- instance id;
- AgentCard URL and extension list;
- task ids and terminal states;
- artifacts produced by each scenario;
- server log path with secrets redacted.

## PTY Protocol

T4 should not try to infer PTY state from an empty conformance stub. It should:

1. Provision or start a live runtime.
2. Create a known PTY session.
3. Write deterministic frames into the session.
4. Join over `pty-ws/v1` as observer and controller.
5. Verify role assignment, keyframe replay, cursor replay, and A2A core
   operation forwarding over WebSocket.

The executor crate carries a T0/T4 bridge proof for the bare-host target:
`host_runtime_pty_ws_supports_multiple_agents_input_and_replay` registers
multiple `RuntimeKind::Host` instances on one host, joins each over
`pty-ws/v1`, forwards controller input through the `PtyBridge` contract,
checks output isolation between host agents, and verifies replay keyframes for
reattach. This is the fast host-target conformance guard; a future live T4
run should reuse the same assertions against a real host daemon.

## Durability Protocol

T5 should use a real SQLite path, not tmpfs-only state:

1. Start management with a dedicated temporary data directory.
2. Run idempotency requests that populate the cache.
3. Stop management cleanly.
4. Restart management against the same data directory.
5. Re-run the idempotency request and assert replay behavior survives restart.

## Release Gates

For v2.0.x:

- T0 and T1 must pass.
- T1 skip count may be non-zero only for buckets A through F.
- D + F must have tracked issues if not resolved.
- T3 must pass for any release advertised as orchestrator-ready.

For v2.1:

- Required-extension enforcement skips must be removed or explicitly re-deferred
  with release-note justification.
- T2 should be mandatory in CI.

For v2.2:

- Multi-tenant enforcement skips must move from B to pass/fail.
- T3 should include tenant-scoped task visibility once semantics are enforced.

For PTY stable promotion:

- T4 must pass and the `pty-ws/v1` spec stability tier can then move from beta
  to stable.

## Reporting Format

Every conformance run should publish:

- markdown report;
- JUnit XML report;
- server log with secrets redacted;
- skip summary grouped by bucket;
- implementation commit under test;
- harness commit under test.

The CI report is pass/fail for non-skipped tests. The release report is
pass/fail plus skip-budget review: an unexpected skip is a release blocker even
if the harness exits `0`.

## Baseline Skip Inventory

The following skip inventory is the reviewed baseline for `77f8f5d`. Future
runs should preserve the bucket labels or explain why a test moved.

| Bucket | Test |
|---|---|
| A | `terminal_completed_shape` |
| A | `terminal_failed_includes_fail_kind` |
| A | `terminal_canceled_sets_terminal_at` |
| A | `terminal_rejected_shape` |
| A | `cancel_task_transitions_to_canceled` |
| A | `extensions/hitl_prompt/input_required_carries_envelope_when_activated` |
| A | `extensions/hitl_prompt/hitl_response_message_accepted_shape` |
| A | `extensions/hitl_prompt/invalid_hitl_response_returns_422` |
| B | `capability_tiers/required_extension_enforced_or_skipped` |
| B | `registration/required_capability_enforcement_v2_deviation` |
| B | `extensions/multi_tenant/tenant_id_echoed_in_get_task` |
| B | `extensions/multi_tenant/tenant_id_invalid_charset_rejected` |
| B | `extensions/runtime/task_metadata_carries_runtime_keys_when_activated` |
| C | `pty_binding/a2a_core_op_over_ws_roundtrips` |
| C | `pty_binding/pty_join_session_assigns_role` |
| C | `pty_binding/replay_from_cursor_returns_keyframe` |
| D | `error_handling/missing_auth_returns_401_when_required` |
| D | `error_handling/rate_limit_returns_429_when_quota_configured` |
| D | `error_handling/forced_5xx_inspection` |
| E | `extensions/idempotency/restart_durability` |
| F | `capability_tiers/deprecated_v1_path_emits_sunset_header` |
