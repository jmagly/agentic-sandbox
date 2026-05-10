# agentic-sandbox v2 Migration Guide

This guide is the canonical reference for moving from the v1 surface
(`/api/v1/...` + legacy WebSocket on port 8121) to the v2 surface, which
realigns the executor around the [Agent2Agent (A2A) protocol](https://a2aproject.github.io/A2A/),
splits the API into three explicit surfaces, and routes per-instance.

Read this in full once before migrating any client. Code samples assume
HTTP+JSON unless noted otherwise.

## Overview

v2 is **additive**. v1 routes still respond exactly as they did in
`2026.5.0`, but every v1 response now carries three deprecation headers:

| Header                | Value                                                                     |
|-----------------------|---------------------------------------------------------------------------|
| `Sunset`              | `Sun, 09 May 2027 00:00:00 GMT` (default; configurable per deployment)    |
| `Deprecated`          | `true`                                                                    |
| `Link`                | `<https://agentic-sandbox.aiwg.io/v2-migration-guide>; rel="successor-version"` |

The default `Sunset` date can be overridden by setting
`AIWG_V1_SUNSET_DATE` on the management server (RFC 7231 IMF-fixdate;
invalid values log a warning and fall back to the default).

**Removal target: v3.0**, not earlier than 12 months after v2.0 GA
(ADR-018). Plan accordingly.

## Surfaces

ADR-022 splits the executor into three surfaces. Pick the right one for
each call site — they have different auth models, audiences, and rate
limits.

| Surface | URL prefix                              | Purpose                                                  |
|---------|-----------------------------------------|----------------------------------------------------------|
| 1 — Admin | `/api/v2/admin/...`                   | Provisioning, registry, retention, capacity. Operator-facing. |
| 2 — A2A   | `/agents/{instance_id}/...`           | Per-instance agent protocol: messages, tasks, push, SSE, AgentCard. Client-facing. |
| 2 — A2A (PTY transport) | `/agents/{instance_id}/sessions/{sid}/attach` (WSS) | Interactive PTY attach — A2A-compatible task with the `pty-ws/v1` binding. |
| 3 — Observability       | `/metrics`, `/healthz`, `/readyz`     | Prometheus + health probes. No auth (typically scrape-controlled by network). |

Hard rule: do not mix surfaces in one call. Admin endpoints never appear
under `/agents/{id}/`, and A2A endpoints never appear under
`/api/v2/admin/`.

## v1 → v2 path map

The canonical map lives in code at
[`management/src/http/compat_v1.rs`](../management/src/http/compat_v1.rs)
(`path_map()`). The table below mirrors it; if the two diverge, the
code is authoritative.

| v1                                          | v2 destination                                                                    |
|---------------------------------------------|-----------------------------------------------------------------------------------|
| `GET    /api/v1/agents`                     | `GET    /api/v2/admin/instances`                                                  |
| `*      /api/v1/vms[...]`                   | `*      /api/v2/admin/instances`                                                  |
| `GET    /api/v1/operations/{id}`            | `GET    /api/v2/admin/operations/{id}`                                            |
| `*      /api/v1/storage/{scope}/{path}`     | `*      /api/v2/admin/storage/{scope}/{path}`                                     |
| `GET    /api/v1/container-images`           | `GET    /api/v2/admin/container-images`                                           |
| `POST   /api/v1/sessions/{id}/dispatch`     | `POST   /agents/{id}/v1/messages:send` (A2A — **semantic shift**, not a rename)   |
| `WS     /api/v1/ws/missions/{id}`           | `GET    /agents/{id}/v1/tasks/{tid}/subscribe` (SSE — **transport shift**)        |
| `*      /api/v1/hitl/{id}`                  | A2A `input-required` task state + `hitl-prompt/v1` extension                      |
| `WS     ws://host:8121/sessions/{id}`       | `WSS    wss://host/agents/{id}/sessions/{sid}/attach` (`pty-ws/v1` binding)       |

Translate carefully: the two flagged "shifts" are not field renames —
they change the protocol model (mission events → A2A task states; raw
WS frames → SSE-encoded `TaskStatusUpdate` / `TaskArtifactUpdate`).

## A2A Surface (Surface 2)

### AgentCard discovery

Every instance publishes a signed AgentCard at:

```
GET https://{host}/agents/{instance_id}/.well-known/agent-card.json
```

The card is JCS-canonicalized JSON signed with JWS (Ed25519). It
declares:

- `name`, `description`, `version`, `protocolVersion`
- `supportedInterfaces` — explicit list of bindings (REST + `pty-ws/v1`)
- `securitySchemes` — auth schemes accepted by this instance
- `capabilities` — including which A2A extensions are advertised /
  required / activated
- `signature` — JWS over the JCS-canonical form

Verification: parse the card, strip the `signature` field, JCS-canonicalize the
remainder, verify the JWS with the issuer's public key. Spec §8 of the
A2A protocol covers signature semantics; see also ADR-018 §AgentCard
signing.

### Core REST operations

All routes are `Content-Type: application/json` unless an extension
overrides.

| Operation                              | Method | Path                                                                 |
|----------------------------------------|--------|----------------------------------------------------------------------|
| Send message (start/continue task)     | POST   | `/agents/{id}/v1/messages:send`                                      |
| Get task                               | GET    | `/agents/{id}/v1/tasks/{tid}`                                        |
| List tasks (cursor pagination, state filter) | GET | `/agents/{id}/v1/tasks?state=&cursor=&limit=`                   |
| Cancel task                            | POST   | `/agents/{id}/v1/tasks/{tid}/cancel`                                 |
| Subscribe (SSE)                        | GET    | `/agents/{id}/v1/tasks/{tid}/subscribe`                              |
| Get extended AgentCard                 | GET    | `/agents/{id}/v1/extendedAgentCard`                                  |
| Create push-notification config        | POST   | `/agents/{id}/v1/tasks/{tid}/pushNotificationConfigs`                |
| Get push-notification config           | GET    | `/agents/{id}/v1/tasks/{tid}/pushNotificationConfigs/{cid}`          |
| List push-notification configs         | GET    | `/agents/{id}/v1/tasks/{tid}/pushNotificationConfigs`                |
| Delete push-notification config        | DELETE | `/agents/{id}/v1/tasks/{tid}/pushNotificationConfigs/{cid}`          |

`messages:send` is the A2A equivalent of v1's `sessions/.../dispatch`.
The request body is an A2A `Message` (parts, role, optional
`referenceTaskId` for follow-ups). The response is an A2A `Task` with
its current state — clients then either poll
`/v1/tasks/{tid}`, subscribe via SSE, or register a push config.

### Activating extensions

A2A extensions are activated per-request by sending the
`A2A-Extensions` header with a comma-separated list of extension URIs.
The server echoes the extensions it actually applied in the response
`A2A-Extensions` header — clients must compare and react if a required
extension is missing.

```
A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/runtime/v1,
                https://agentic-sandbox.aiwg.io/extensions/idempotency/v1
```

v2.0 ships five extensions:

| URI suffix                  | Status in v2.0           | Spec                                                                 |
|-----------------------------|--------------------------|----------------------------------------------------------------------|
| `runtime/v1`                | Declared `required: true` in AgentCard; **enforcement deferred** to v2.1 (see ADR-022 deviation note) | [`docs/contracts/extensions/runtime/v1/spec.md`](contracts/extensions/runtime/v1/spec.md) |
| `idempotency/v1`            | Declared `required: true`; activate to enable cache | [`docs/contracts/extensions/idempotency/v1/spec.md`](contracts/extensions/idempotency/v1/spec.md) |
| `hitl-prompt/v1`            | Optional                 | [`docs/contracts/extensions/hitl-prompt/v1/spec.md`](contracts/extensions/hitl-prompt/v1/spec.md) |
| `multi-tenant/v1`           | Beta — **shape declared in v2.0, enforcement in v2.2** (ADR-013) | [`docs/contracts/extensions/multi-tenant/v1/spec.md`](contracts/extensions/multi-tenant/v1/spec.md) |
| `pty-extensions/v1`         | Optional (paired with `pty-ws/v1`) | [`docs/contracts/extensions/pty-extensions/v1/spec.md`](contracts/extensions/pty-extensions/v1/spec.md) |

Extension URIs and governance: ADR-019. Required-but-deferred is an
intentional state: the AgentCard signals the eventual contract, even
when the executor temporarily skips enforcement — clients that
implement the extension correctly today won't have to change when
enforcement lands.

## Admin Surface (Surface 1)

New paths live under `/api/v2/admin/*`. The full OpenAPI is in
[`docs/contracts/admin-api.openapi.yaml`](contracts/admin-api.openapi.yaml).

Headline operations:

- `GET    /api/v2/admin/instances` — list registered instances
- `POST   /api/v2/admin/instances` — provision new instance
- `GET    /api/v2/admin/instances/{id}` — instance detail
- `DELETE /api/v2/admin/instances/{id}` — decommission (force flag controls disk wipe)
- `GET    /api/v2/admin/operations/{id}` — long-running operation status
- `*      /api/v2/admin/storage/{scope}/{path}` — global / inbox storage
- `GET    /api/v2/admin/container-images` — curated image list for the dashboard

**Auth**: v1 admin tokens (`Authorization: Bearer ...` against the
configured admin secret) continue to work via the compat shim. The v2.0
OpenAPI declares mTLS + Unix-domain-socket peer creds as the future
target (enforced in v2.x); for v2.0 GA, bearer auth is the supported
path.

## PTY Surface (Surface 2 transport variant)

The legacy v1 PTY WebSocket lives on its own port (`ws://host:8121/sessions/{id}`)
and uses an ad-hoc frame protocol. v2 moves PTY attach onto the A2A
host, on the standard HTTPS port, using the `pty-ws/v1` binding.

```
wss://{host}/agents/{instance_id}/sessions/{session_id}/attach
```

Wire format and frame schema:
[`docs/contracts/bindings/pty-ws/v1/spec.md`](contracts/bindings/pty-ws/v1/spec.md)
and `frames.schema.json` in the same directory. Activate
`pty-extensions/v1` to negotiate extras like resize floors and
explicit reattach semantics.

The v1 PTY endpoint is deprecated by association — the `Sunset` /
`Link` headers above apply only to HTTP responses, but the same v3.0
removal window covers the legacy WS port. Migrate PTY clients alongside
HTTP clients.

## Auth changes

Summary of what's new in the AgentCard and what the executor enforces
today:

| Concern                  | AgentCard declares                                                | v2.0 enforces                              |
|--------------------------|-------------------------------------------------------------------|--------------------------------------------|
| Bearer tokens            | `securitySchemes: { bearer: { type: http, scheme: bearer } }`     | Yes — v1 admin tokens accepted             |
| mTLS                     | Declared in `securitySchemes` for admin & A2A surfaces            | **Deferred to v2.x** (ADR-015)             |
| Unix peer creds (local)  | Declared for admin surface on Unix-socket transport               | **Deferred to v2.x**                       |
| JWS-signed AgentCard     | `signature` field                                                 | Yes — sign + serve; clients should verify  |

Clients verifying AgentCard signatures today will keep working as the
declared schemes graduate from "declared" to "enforced" without further
client changes.

## Sunset timeline

| Date / version                  | What happens                                                              |
|---------------------------------|---------------------------------------------------------------------------|
| v2.0 GA (`<release-date>`)      | v1 surfaces keep responding; `Sunset` + `Deprecated` + `Link` headers on every v1 response |
| `Sun, 09 May 2027 00:00:00 GMT` | Default `Sunset` date (configurable per deployment via `AIWG_V1_SUNSET_DATE`) |
| v3.0 (≥12 months after v2.0 GA) | v1 routes removed entirely (ADR-018)                                      |

Operators monitoring `aiwg_v1_path_requests_total{path="..."}` in
Prometheus can see who's still on v1 and target migration work before
v3.0.

## Conformance harness

Once your deployment claims v2 support, run the conformance harness
against it before you tell clients to switch:

```bash
agentic-sandbox-conformance \
  --executor-url https://your-deployment/ \
  --token <bearer> \
  --report-format markdown,junit \
  --report-out conformance.report.md
```

Exit status `0` = pass. Anything else means at least one contract
assertion failed; the report explains which surface, which test, and
the observed vs expected payload.

Install / source:
[`roctinam/agentic-sandbox-conformance`](https://git.integrolabs.net/roctinam/agentic-sandbox-conformance).
Tracked as deliverable #217 in the v2 epic.

## Common migration mistakes

A short list of footguns picked up during pre-GA testing:

- **Treating `messages:send` as a rename of `dispatch`**. The response
  is a `Task`, not a synchronous result — wire your client to poll
  `/v1/tasks/{tid}` or subscribe before assuming completion.
- **Ignoring the echoed `A2A-Extensions` response header**. If you
  requested `idempotency/v1` and the server did not echo it back, your
  request was processed without idempotency. Treat that as a failed
  precondition.
- **Re-using the v1 mission ID as the A2A task ID**. They live in
  different namespaces. Map them at the migration boundary, do not
  alias.
- **Calling admin endpoints with a per-agent token**. Admin surface
  takes the admin bearer; A2A surface takes the per-instance scheme
  declared in that instance's AgentCard.

## References

- [ADR-018 — A2A as base protocol](../.aiwg/architecture/adr/ADR-018-a2a-as-base-protocol.md)
- [ADR-019 — Extension URI scheme and governance](../.aiwg/architecture/adr/ADR-019-extension-uri-scheme-and-governance.md)
- [ADR-020 — PTY custom protocol binding](../.aiwg/architecture/adr/ADR-020-pty-custom-protocol-binding.md)
- [ADR-021 — `a2a-rs` as wire dependency](../.aiwg/architecture/adr/ADR-021-a2a-rs-as-wire-dependency.md)
- [ADR-022 — Three-surface architecture](../.aiwg/architecture/adr/ADR-022-three-surface-architecture.md)
- [v2 executor contract SAD](../.aiwg/architecture/v2-executor-contract-sad.md)
- [Contracts directory](contracts/) — OpenAPI, extension specs, binding specs
- [`management/src/http/compat_v1.rs`](../management/src/http/compat_v1.rs) — canonical v1→v2 path map (code is authoritative)
