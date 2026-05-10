# A2A Extension: `multi-tenant/v1`

| Field | Value |
|---|---|
| **URI** | `https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1` |
| **Stability tier** | `beta` (v2.0, declared shape only) — graduates to `stable` in v2.2 (enforced) |
| **Spec version** | 1.0.0 |
| **Status** | Beta. Wire shape is frozen for v2.0; semantics activate in v2.2. |
| **Editors** | agentic-sandbox maintainers |
| **Reference impl** | `agentic-sandbox-executor` crate, `extensions::multi_tenant` module |
| **Related ADRs** | ADR-013, ADR-018, ADR-019, ADR-022 |
| **Related use cases** | UC-009 (Multi-Orchestrator Concurrent Dispatch) |

> **Tier banner — beta (v2.0)**
> This extension is in **beta** in agentic-sandbox v2.0: the wire shape (field placement, response envelopes, AgentCard advertisement) is frozen and MUST be implemented exactly as specified, but the **semantics are not enforced**. Sandboxes accept `tenant_id` and treat all values as the implicit `"default"` tenant. Quota responses (`429 + Retry-After`) are documented but MUST NOT be emitted by quota machinery in v2.0.
>
> **Tier banner — stable (v2.2)**
> In agentic-sandbox v2.2 this extension graduates to **stable**: `tenant_id` actually scopes resource visibility, per-tenant rate limits and concurrent-mission caps are enforced, cross-tenant requests return `404` (never `403`), and Prometheus metrics gain a per-tenant label. v2.2 is a behavioral upgrade — no wire-shape change relative to v1.

---

## 1. Conformance language

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** in this document are to be interpreted as described in [RFC 2119] and [RFC 8174] when, and only when, they appear in all capitals.

[RFC 2119]: https://www.rfc-editor.org/rfc/rfc2119
[RFC 8174]: https://www.rfc-editor.org/rfc/rfc8174
[RFC 6585]: https://www.rfc-editor.org/rfc/rfc6585

---

## 2. Motivation and scope

agentic-sandbox is designed to host concurrent dispatches from multiple orchestrators (e.g. AIWG-prod and AIWG-dev sharing a deployment, or AIWG alongside a third-party orchestrator such as a smolagents adapter). Per ADR-013, multi-tenancy is declared in v2.0 — so that orchestrator integrators can adopt the wire contract immediately — and enforced in v2.2 — so that the substantial isolation engineering (per-tenant rate limits, per-tenant WS channels, cross-tenant existence-leak prevention) does not block v2.0 release.

This extension defines:

1. The `tenant_id` field on `Message.metadata` (carried on every A2A `message/send` and `message/stream` request).
2. The shape of the `429 Too Many Requests` response envelope returned when a tenant exceeds quota (declared in v2.0, emitted in v2.2).
3. The AgentCard advertisement under `capabilities.extensions[]`.

Out of scope for this extension:

- **Authentication and authorization.** `tenant_id` is an opaque routing/namespacing token; it is not a credential and MUST NOT be used as the sole basis for authorization. Token issuance, validation, and binding of `tenant_id` to identity belong to the auth layer (see ADR-015).
- **Per-tenant URI mounts** (`/api/v2/tenants/{tid}/...`). ADR-013 reserves a parallel URI path; that is a deployment concern. This extension carries the tenant identifier *in band* via `Message.metadata` so that it travels with every message regardless of URI mount.
- **Storage namespacing** (e.g. agentshare paths under `/inbox/{tenant_id}/...`). Specified in the runtime extension and operational deployment docs, not here.
- **Cost or token budgeting per tenant.** Reserved for `cost-budget/v1` (ADR-019).

---

## 3. AgentCard advertisement

A sandbox executor that supports this extension MUST include the following entry in its AgentCard `capabilities.extensions[]` array:

```json
{
  "uri": "https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1",
  "description": "Carries an opaque tenant_id on Message.metadata; declared in v2.0, enforced in v2.2.",
  "required": false,
  "params": {
    "enforcement": "declared",
    "default_tenant": "default",
    "quota_response": {
      "status": 429,
      "header": "Retry-After"
    }
  }
}
```

### 3.1 `params` schema

| Field | Type | Required | Description |
|---|---|---|---|
| `enforcement` | `"declared"` \| `"enforced"` | yes | `"declared"` in v2.0 (no isolation); `"enforced"` in v2.2 (full isolation). Clients MUST treat the value as advisory only — actual semantics are fixed by the executor's deployed version. |
| `default_tenant` | string | yes | The tenant identifier the sandbox uses when no `tenant_id` is supplied. MUST be `"default"` for v1. |
| `quota_response.status` | integer | yes | HTTP status returned on quota exhaustion. MUST be `429`. |
| `quota_response.header` | string | yes | HTTP header carrying retry guidance. MUST be `"Retry-After"`. |

The `params` object MUST be present so that orchestrators can detect tier without reading the spec. `required` MUST be `false` — this extension is opt-in for both directions: orchestrators that omit `tenant_id` are accepted as the `"default"` tenant.

### 3.2 Activation header

Per A2A extension governance, clients that intend to populate `tenant_id` SHOULD send:

```http
A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1
```

The executor MUST echo activated extensions in its response `A2A-Extensions` header. Activation is advisory in v2.0 (the field is read whether activated or not). In v2.2, executors MAY require activation before honoring `tenant_id` — but this is a deployment policy, not a spec requirement.

---

## 4. Wire format

### 4.1 `Message.metadata.tenant_id`

A2A's `Message` object exposes a free-form `metadata` map. This extension reserves the key `tenant_id` on that map.

```json
{
  "kind": "message",
  "messageId": "01HM4Z9KQH3...",
  "role": "user",
  "parts": [ /* ... */ ],
  "metadata": {
    "tenant_id": "aiwg-prod"
  }
}
```

Constraints (formal JSON Schema in [`metadata.schema.json`](./metadata.schema.json)):

- **Type**: string.
- **Length**: 1 to 128 characters.
- **Charset**: `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$` — printable ASCII, no whitespace, no slashes (so it is safe in URI segments and Prometheus labels without escaping).
- **Opacity**: the value is opaque to A2A and to this extension. It MUST NOT be parsed for substructure by sandboxes or peers; it MAY be parsed by the issuing operator for their own bookkeeping.
- **Reserved value**: `"default"` is the implicit tenant when the field is absent. Clients MAY send `"default"` explicitly; the result is identical to omitting the field.
- **Stability**: the value SHOULD remain stable for the lifetime of an orchestrator deployment. Operators that rotate `tenant_id` lose the ability for v2.2 to correlate prior missions with the new identifier.

If `tenant_id` is present but does not satisfy the schema, the executor MUST respond with HTTP `400 Bad Request` and a JSON-RPC error of code `-32602` ("Invalid params"), naming `metadata.tenant_id` as the offending field.

### 4.2 Propagation

When a sandbox emits status updates, artifacts, or any A2A message *back* to a client, it MUST echo the `tenant_id` it received on the originating request, on the `metadata` map of every emitted `Message` and `TaskStatusUpdateEvent`. This lets clients correlate streamed events to the originating tenant without re-deriving from URI or token.

In v2.0 this is a courtesy echo (executor still does no isolation). In v2.2 it is mandatory for client-side filtering correctness when a single client subscribes to multiple tenants.

### 4.3 Quota response envelope

When (in v2.2) a request is rejected because the tenant has exhausted its rate budget or concurrent-mission cap, the executor MUST return:

```http
HTTP/1.1 429 Too Many Requests
Retry-After: 12
Content-Type: application/json

{
  "jsonrpc": "2.0",
  "id": "<original request id>",
  "error": {
    "code": -32029,
    "message": "Tenant quota exceeded",
    "data": {
      "tenant_id": "aiwg-prod",
      "quota": "rate",
      "retry_after_seconds": 12,
      "limit": 100,
      "window_seconds": 60
    }
  }
}
```

Formal schema in [`quota-response.schema.json`](./quota-response.schema.json).

Constraints:

- HTTP status MUST be `429` (per [RFC 6585]).
- `Retry-After` header MUST be present with an integer seconds value (delta-seconds form, per RFC 7231 §7.1.3).
- `error.code` MUST be `-32029` (a sandbox-reserved JSON-RPC application error code for quota; chosen to fall outside A2A and JSON-RPC reserved ranges).
- `error.data.tenant_id` MUST equal the request's `tenant_id` (or `"default"` if absent).
- `error.data.quota` MUST be one of `"rate"` (token bucket on dispatch rate) or `"concurrency"` (cap on non-terminal missions).
- `error.data.retry_after_seconds` MUST equal the value of the `Retry-After` header.

In v2.0, the quota machinery is not active and this envelope MUST NOT be emitted from quota enforcement. (It MAY still appear from upstream load-shedding or proxy-imposed rate limits, but those are not governed by this extension.)

---

## 5. Behavior — v2.0 (declared)

This subsection specifies the v2.0 behavior of an executor advertising this extension. v2.0 is the declared-shape tier; it accepts the wire format without enforcing semantics.

1. The executor MUST accept `Message.metadata.tenant_id` on every inbound `message/send` and `message/stream` request.
2. The executor MUST validate the value against the schema in §4.1 and reject malformed values with `400` / `-32602`.
3. The executor MUST treat all valid `tenant_id` values as equivalent to the implicit `"default"` tenant — that is, no resource scoping, no per-tenant rate limit, no per-tenant concurrency cap.
4. The executor MUST echo the received `tenant_id` (or `"default"` if absent) back on outbound messages per §4.2.
5. The executor MUST advertise the extension in its AgentCard with `params.enforcement = "declared"` per §3.
6. The executor MUST NOT emit the `429 + Retry-After` quota envelope from quota machinery. Quota machinery is inactive.
7. Cross-tenant queries (e.g. tenant A asking for a mission registered by tenant B) **succeed** in v2.0 — there is no isolation. This is intentional; integrators MUST NOT rely on isolation at this tier.
8. Prometheus metrics MAY include a `tenant_id` label, but if so the label value will be `"default"` for all series in v2.0.

---

## 6. Behavior — v2.2 (enforced) — RESERVED

This subsection is **reserved**: it specifies the v2.2 behavior in advance so that v2.0 integrators can plan, but is not part of the v2.0 conformance contract. Conformance harnesses for v2.0 MUST NOT assert any of the requirements in this section.

When the executor advertises `params.enforcement = "enforced"`:

1. The executor MUST scope mission visibility by `tenant_id`: `tasks/get`, `tasks/list`, and event-stream subscriptions return only missions with a matching `tenant_id`.
2. The executor MUST return `404 Not Found` (JSON-RPC error `-32001` "Task not found") for any cross-tenant lookup. It MUST NOT return `403 Forbidden`. This is a deliberate choice to prevent existence-leak (see §8.1).
3. The executor MUST enforce a per-tenant token-bucket rate limit on dispatch (default: 100 dispatches/min) and emit the `429 + Retry-After` envelope per §4.3 when exceeded.
4. The executor MUST enforce a per-tenant leaky-bucket cap on concurrent non-terminal missions (default: 10) and emit the same `429` envelope with `quota: "concurrency"` when exceeded.
5. The executor MUST label every Prometheus metric series concerning missions, dispatch, events, or quota with `tenant_id`.
6. Per-tenant WS channels: the executor MUST scope event delivery so that one tenant's events never appear on another tenant's WS subscription. (Wire path for the WS channel is governed by ADR-013 and the runtime/v1 extension; this extension only requires the scoping property.)
7. AgentCard `params.enforcement` MUST be `"enforced"`.
8. All v2.0 wire-format requirements (§4) remain in force unchanged.

A v2.2 executor MUST NOT downgrade silently to v2.0 behavior; if isolation cannot be guaranteed at startup (e.g. backing store does not have `tenant_id` columns), the executor MUST refuse to start.

---

## 7. Conformance test scenarios

Conformance tests are split into v2.0 (declared, asserted by the v2.0 harness) and v2.2 (reserved, not asserted in the v2.0 harness).

### 7.1 v2.0 — declared (asserted)

| ID | Scenario | Expected |
|---|---|---|
| MT-D-01 | Send `message/send` with `metadata.tenant_id = "aiwg-prod"`. | `200 OK`; response message echoes `metadata.tenant_id = "aiwg-prod"`. |
| MT-D-02 | Send `message/send` with no `tenant_id` field. | `200 OK`; response echoes `metadata.tenant_id = "default"`. |
| MT-D-03 | Send `message/send` with `metadata.tenant_id = ""` (empty string). | `400 Bad Request`; JSON-RPC error `-32602`; `data.field = "metadata.tenant_id"`. |
| MT-D-04 | Send `message/send` with `metadata.tenant_id = "has spaces"`. | `400 Bad Request`; JSON-RPC error `-32602`. |
| MT-D-05 | Send `message/send` with `metadata.tenant_id` exceeding 128 characters. | `400 Bad Request`; JSON-RPC error `-32602`. |
| MT-D-06 | Two clients dispatch missions with different `tenant_id` values; both query `tasks/list`. | Both clients see **both** missions. (No isolation in v2.0 — this is a positive assertion that v2.0 does not isolate.) |
| MT-D-07 | Fetch AgentCard. | `capabilities.extensions[]` contains the extension URI with `params.enforcement = "declared"`, `params.default_tenant = "default"`, `params.quota_response = {status: 429, header: "Retry-After"}`. |
| MT-D-08 | Drive the executor to a high dispatch rate (e.g. 1000/min). | Executor MUST NOT emit a `429 + Retry-After` from this extension's quota machinery. (Upstream proxy 429s are out of scope.) |
| MT-D-09 | Subscribe to event stream; another tenant emits events. | Subscriber sees the other tenant's events. (Confirms no isolation.) |

### 7.2 v2.2 — enforced (RESERVED — not asserted in v2.0 harness)

| ID | Scenario | Expected (v2.2 only) |
|---|---|---|
| MT-E-01 | Tenant A queries tenant B's mission by ID. | `404 Not Found`; JSON-RPC error `-32001`. (NOT `403`.) |
| MT-E-02 | Tenant A subscribes to events; tenant B dispatches a mission. | Tenant A sees no events from tenant B's mission. |
| MT-E-03 | Tenant A exceeds dispatch rate. | `429`; `Retry-After` header present; `error.data.quota = "rate"`. |
| MT-E-04 | Tenant A reaches concurrent-mission cap. | `429`; `error.data.quota = "concurrency"`. |
| MT-E-05 | Tenant B dispatches concurrently with tenant A's quota exhaustion. | Tenant B's dispatches succeed unaffected. |
| MT-E-06 | Scrape `/metrics`. | Mission/dispatch/event/quota series carry a `tenant_id` label. |
| MT-E-07 | Start executor with backing store lacking `tenant_id` columns. | Executor refuses to start; emits clear error. |
| MT-E-08 | Fetch AgentCard. | `params.enforcement = "enforced"`. |

The v2.0 harness MUST tag MT-E-* tests as `skipped (reserved-v2.2)` and MUST NOT count them as failures.

---

## 8. Security considerations

### 8.1 Cross-tenant existence leakage — `404` vs `403`

When v2.2 enforces isolation, cross-tenant lookups MUST return `404 Not Found`, not `403 Forbidden`. The rationale:

- `403` reveals that *something exists at that identifier* but the caller is not authorized. An attacker enumerating `task_id` values can use a `403`/`404` distinction to map another tenant's mission inventory.
- `404` is indistinguishable from "no such mission ever existed". The attacker learns nothing about other tenants.

This follows the pattern used by GitHub (private-repo URLs return 404 to non-members), AWS S3 (bucket existence is masked behind 404 for non-owners on certain operations), and is documented in OWASP ASVS V8 (data protection) and the IETF httpapi BCPs on enumeration resistance.

The same rule applies to event subscriptions in v2.2: requesting events for a cross-tenant mission MUST yield `404`, not a successful subscription with empty event delivery (which would also leak existence by latency-side-channel).

### 8.2 `tenant_id` is not a credential

`tenant_id` is an opaque routing token. It carries no authentication and no authorization. In particular:

- A request whose token claims tenant `aiwg-prod` but whose `Message.metadata.tenant_id` says `aiwg-dev` MUST be rejected by the auth layer, not by this extension. This extension does not specify how to detect such a mismatch — that is the auth layer's job (see ADR-015, future). The extension only specifies the *carrier* for the value.
- Sandboxes MUST NOT make trust decisions based on the value of `tenant_id` alone. The auth layer establishes which tenants a token may speak for; this extension only governs which tenant a *request* refers to.
- Operators MUST NOT use `tenant_id` for billing or compliance attribution unless there is a separate auth-bound mapping from authenticated identity to tenant. Otherwise an unprivileged client may simply set `tenant_id` to any string.

In v2.0, where there is no enforcement, treating `tenant_id` as a credential is doubly dangerous: there is no validation at all, and there is no isolation. v2.0 deployments operating in shared mode should therefore be considered single-tenant for all security purposes.

### 8.3 Charset and injection

The `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$` charset prevents:

- URI-segment injection (no `/`, no `%`).
- Log-injection (no newlines, no control chars).
- Prometheus-label-injection (no `"`, no `{`, no `}`).
- Filesystem-path injection in storage namespacing (no `..`, no `/`).

Implementations MUST NOT relax this constraint. A wider charset is a major-version change (`multi-tenant/v2`).

### 8.4 Side channels in v2.0

Because v2.0 does not isolate, a client populating `tenant_id` correctly is still observable to all other clients on the same deployment. This includes:

- Mission IDs (via `tasks/list`).
- Event payloads (via shared subscriptions).
- Metric series (via `/metrics`).

Operators who need any isolation in v2.0 MUST deploy separate sandbox instances per tenant (see ADR-022 three-surface architecture). This extension's v2.0 tier is for *forward-compatible wire shape*, not for security.

### 8.5 Quota response disclosure

The v2.2 quota envelope reveals the configured `limit` and `window_seconds`. Operators who consider these values sensitive MAY redact them from `error.data`, in which case `limit` and `window_seconds` MAY be omitted; `tenant_id`, `quota`, and `retry_after_seconds` MUST always be present.

---

## 9. Migration from v2.0 to v2.2

This is a **behavioral upgrade with no wire-shape change**. Orchestrators that already populate `tenant_id` correctly in v2.0 will start receiving isolation when their sandbox is upgraded to v2.2; orchestrators that omit `tenant_id` will continue to be treated as the `"default"` tenant.

Recommended actions for orchestrator authors during the v2.0 → v2.2 window:

1. Populate `tenant_id` on every outbound `Message` from the start. Use a stable identifier (e.g. a deployment-scoped UUID or a human-readable slug like `aiwg-prod`).
2. Echo the received `tenant_id` on inbound correlation, and filter your own subscriptions defensively even though v2.0 does not isolate.
3. Treat the `429 + Retry-After` envelope as a possibility from day one — even if v2.0 never emits it, your client should already have backoff logic so that v2.2 deployment is a no-op on the client side.
4. Watch for `params.enforcement` changing from `"declared"` to `"enforced"` on the AgentCard; that signals you are now talking to v2.2.

A v2.0 client that hard-codes `tenant_id = "default"` continues to work indefinitely on both v2.0 and v2.2 (it simply gets the default tenant's resources and shares one bucket with any other defaulted client).

---

## 10. Examples

- [`examples/declared-v20.json`](./examples/declared-v20.json) — a v2.0 `message/send` request with `tenant_id` populated, demonstrating that the field is accepted but not enforced.
- [`examples/enforced-v22-quota-exceeded.http`](./examples/enforced-v22-quota-exceeded.http) — a v2.2 reserved example showing the `429 + Retry-After` envelope on quota exhaustion. **Not emitted in v2.0.**

---

## 11. Dependencies

This extension has no hard dependencies on other extensions. It is recommended to deploy alongside:

- `runtime/v1` — sandboxes typically advertise both.
- `idempotency/v1` — idempotency keys are independent of tenancy but are usually scoped per-tenant in v2.2.

This extension does not require activation of any other extension to function.

---

## 12. Change log

| Spec version | Date | Notes |
|---|---|---|
| 1.0.0 | 2026-05-09 | Initial release. Tier `beta` (declared) for v2.0; reserves `stable` (enforced) tier for v2.2. |
