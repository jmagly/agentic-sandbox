# A2A Extension: `idempotency/v1`

## Identity

| Field | Value |
|---|---|
| Extension URI | `https://agentic-sandbox.aiwg.io/extensions/idempotency/v1` |
| Spec version | `1.0.0` |
| Stability tier | `stable` |
| Status | Accepted (2026-05-09) |
| Authors | agentic-sandbox maintainers (`roctinam/agentic-sandbox`) |
| Reference implementation | `agentic-sandbox-executor` crate, module `extensions::idempotency` |
| Depends on | A2A core (JSON-RPC over HTTP transport) |
| Related ADRs | ADR-008 (Idempotency-Key dispatch contract), ADR-019 (extension URI scheme), ADR-014 (outbox / TaskStore) |

This document uses the key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** as described in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) and [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174).

## 1. Summary

This extension defines server-side request deduplication for A2A mutating operations. Servers store the tuple `(message_id, request_hash, response)` for a 24-hour TTL keyed on the A2A core `Message.message_id` field (client-generated). A retry that re-uses the same `message_id` with the same canonicalized request body returns the cached response with the response header `Idempotent-Replayed: true`. A re-use of the `message_id` with a different canonicalized body within TTL returns HTTP 422 with the error code `IDEMPOTENCY_KEY_REUSED_WITH_DIFFERENT_PAYLOAD`. Past TTL the request is treated as new.

The cache MUST survive executor restarts; in the agentic-sandbox reference implementation it is persisted in the SQLite-backed `TaskStore` shared with the mission outbox.

## 2. Motivation

A2A `SendMessage` and `SendStreamingMessage` are mutating: they create or advance Tasks. Without deduplication, a client retry after a dropped HTTP response (or any transient failure between request acceptance and client-visible acknowledgement) silently produces duplicate work — duplicate missions, duplicate tool calls, duplicate side effects. ADR-008 originally specified an HTTP-level `Idempotency-Key` header. ADR-018 reframed the design under A2A: the equivalent client-supplied identifier already exists as `Message.message_id`. This extension binds dedup semantics to that field.

Industry precedent: Stripe, Square, OpenAI Responses, and the IETF draft `draft-ietf-httpapi-idempotency-key-header` all converge on the `(key, request_hash, cached_response)` model with a TTL in the 24h range. We adopt the same model, expressed via A2A's existing field.

## 3. Activation

### 3.1 AgentCard declaration

A server supporting this extension MUST advertise it in `AgentCard.capabilities.extensions[]`:

```json
{
  "uri": "https://agentic-sandbox.aiwg.io/extensions/idempotency/v1",
  "description": "Server-side request deduplication keyed on Message.message_id with 24h TTL and payload-hash binding.",
  "required": true,
  "params": {
    "ttl_seconds": 86400,
    "canonicalization": "jcs-rfc8785",
    "hash_algorithm": "sha-256",
    "max_cache_entries": 100000
  }
}
```

For production AgentCards in agentic-sandbox v2.0, this extension is **REQUIRED** (`required: true`). Clients that do not activate it MUST be rejected at request time per A2A's required-extension rules.

### 3.2 Activation header

Clients MUST send:

```
A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/idempotency/v1
```

(comma-separated with any other activated extensions). Servers MUST echo the URI in the response `A2A-Extensions` header on every response governed by the extension.

### 3.3 Params schema

```yaml
params:
  ttl_seconds:        # integer, REQUIRED. TTL of the cache entry in seconds. Default 86400 (24h). Implementations MAY honor a configured override; spec-conformant value is 86400.
  canonicalization:   # string enum, REQUIRED. Currently MUST be "jcs-rfc8785".
  hash_algorithm:     # string enum, REQUIRED. Currently MUST be "sha-256".
  max_cache_entries:  # integer, OPTIONAL. Server-side LRU bound. Default 100000. Eviction does not violate conformance; clients MUST NOT depend on TTL alone.
```

## 4. Protocol

### 4.1 Scope

The extension applies to A2A core mutating operations:

- `message/send` (`SendMessage`)
- `message/stream` (`SendStreamingMessage`)

It does NOT apply to:

- `tasks/get`, `tasks/list`, `tasks/cancel` (read-only or terminal-only state changes that are themselves idempotent by Task ID)
- A2A core lifecycle methods that operate on already-existing Task IDs

Other A2A extensions MAY declare additional methods in scope by referencing this extension and listing them.

### 4.2 Required client behavior

A client MUST:

1. Generate `Message.message_id` per A2A core: a value the client treats as unique per logical request. The value MUST be either a UUIDv7 (RFC 9562) or a 128-bit cryptographically random value rendered as a UUID-shaped string, encoded as the canonical UUID form (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`). UUIDv4 is acceptable; lower-entropy schemes MUST NOT be used (see §7.2).
2. Re-use the **same** `message_id` for any retry of the **same** logical request. Generating a fresh `message_id` per retry attempt defeats deduplication and is a client bug.
3. Send the activation header (`A2A-Extensions: …/idempotency/v1`) on every in-scope request.

### 4.3 Required server behavior

On receipt of an in-scope request, the server MUST:

1. **Compute `request_hash`**:
   - Canonicalize the JSON-RPC `params` value (the A2A `MessageSendParams` object) per [RFC 8785 JSON Canonicalization Scheme (JCS)](https://www.rfc-editor.org/rfc/rfc8785). Note: the canonicalization input is `params` only — NOT the JSON-RPC envelope (`id`, `jsonrpc`, `method` are excluded).
   - The `Message.message_id` field MUST be excluded from the canonicalization input. (It IS the lookup key; including it would tautologically bind it to itself.)
   - Compute `request_hash = SHA-256(JCS(params \ {message.message_id}))`. Encode as lowercase hex.
2. **Look up `(message_id, request_hash)` in the cache**:
   - **Hit, same hash, within TTL** → return the cached response unchanged (status, headers, body), and add response header `Idempotent-Replayed: true`. The server MUST NOT re-execute the operation.
   - **Hit, different hash, within TTL** → return HTTP 422 with JSON-RPC error `code = -32422` (numeric — see §4.5) and string code `IDEMPOTENCY_KEY_REUSED_WITH_DIFFERENT_PAYLOAD`. The original cached response is retained; this collision attempt MUST NOT overwrite it.
   - **Hit, expired (timestamp older than TTL)** → treat as miss; proceed.
   - **Miss** → process the request normally; on completion, atomically insert `(message_id, request_hash, response, inserted_at)`. Both successful and failed responses are cached (4xx and 5xx included) so retries return the original outcome rather than re-executing into a different failure mode.
3. **Set response headers** on all in-scope responses:
   - `A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/idempotency/v1`
   - `Idempotent-Replayed: true` if the response was served from cache; otherwise the header MUST be omitted (not `false`).
4. **Persist cache entries durably**. The cache MUST survive process restart, container restart, and host reboot. In the agentic-sandbox reference implementation, entries are stored in the same SQLite database as the A2A `TaskStore` and the mission event outbox (ADR-014), in a dedicated `idempotency_cache` table.

### 4.4 Cache entry schema

Reference implementation table:

```sql
CREATE TABLE idempotency_cache (
    message_id      TEXT NOT NULL PRIMARY KEY,
    request_hash    TEXT NOT NULL,            -- lowercase hex SHA-256
    response_status INTEGER NOT NULL,         -- HTTP status code
    response_headers BLOB NOT NULL,           -- JSON object
    response_body   BLOB NOT NULL,            -- raw response bytes
    inserted_at     INTEGER NOT NULL          -- unix epoch seconds
);
CREATE INDEX idx_idempotency_inserted_at ON idempotency_cache(inserted_at);
```

Eviction: TTL-based (`inserted_at + ttl_seconds < now`) is REQUIRED. LRU bound at `max_cache_entries` is RECOMMENDED for memory/disk discipline. Both MAY be lazy (evicted on next read of the same key, plus a sweep loop); an entry within TTL but evicted by LRU MUST be treated as a cache miss.

### 4.5 Error encoding

Errors are returned per JSON-RPC 2.0 with an extension-specific error object:

```json
{
  "jsonrpc": "2.0",
  "id": "<request id>",
  "error": {
    "code": -32422,
    "message": "Idempotency key reused with different payload",
    "data": {
      "code": "IDEMPOTENCY_KEY_REUSED_WITH_DIFFERENT_PAYLOAD",
      "message_id": "<the conflicting id>",
      "original_request_hash": "<lowercase hex sha-256 of cached body>",
      "submitted_request_hash": "<lowercase hex sha-256 of new body>"
    }
  }
}
```

HTTP status code for this error MUST be 422. The numeric `-32422` is reserved by this extension within the JSON-RPC implementation-defined server-error band (`-32000` to `-32099` per JSON-RPC 2.0 reserves that band for transport-level errors; we use the unallocated `-32422` outside that band, mirroring the HTTP status for operator clarity). Implementations MUST surface the canonical string code (`IDEMPOTENCY_KEY_REUSED_WITH_DIFFERENT_PAYLOAD`) in `error.data.code` so clients can dispatch on a stable identifier independent of numeric value.

The full canonical error code table is published alongside this spec at `error-codes.json`.

## 5. Canonicalization rules (JCS)

Implementations MUST canonicalize per RFC 8785. Specifically:

1. Object keys are sorted lexicographically by UTF-16 code-unit value.
2. Strings are emitted with RFC 8785 escaping rules (no unnecessary escapes, mandatory escapes for `"`, `\`, and U+0000–U+001F).
3. Numbers are serialized per the ECMA-404 / RFC 8785 number-formatting rules: integers without trailing `.0`, floats in shortest-roundtrip form, no `+` on positive exponents.
4. No trailing whitespace; no insignificant whitespace between tokens.
5. UTF-8 byte output.

Before hashing, the server MUST remove the path `params.message.messageId` (A2A core field name in JSON-RPC payload is `messageId` per A2A schema; the extension URI uses the underscore form `message_id` only in prose). All other fields — including `metadata`, `extensions`, `referenceTaskIds`, `parts`, `taskId`, `contextId`, and any nested objects — are part of the canonicalization input.

The hash is `request_hash = lowercase_hex(SHA-256(canonicalized_bytes))`.

## 6. Conformance scenarios

The numbered scenarios below correspond to the acceptance criteria from `UC-006` and form the conformance harness's required test cases.

### 6.1 Scenario AC-1 — replay with identical body

**Setup**: client sends `message/send` with `Message.message_id = M1` and body B. Server returns 202 (or analogous A2A `SendMessageResponse`).

**Action**: client retries with the same `M1` and the same B (modulo whitespace and key ordering — JCS makes the comparison normalize).

**Expected**:
- Status code: identical to first response.
- Body: byte-for-byte identical to first response.
- Header `Idempotent-Replayed: true` MUST be present on the replay.
- Server MUST NOT re-execute side effects (no second Task created, no second tool call, no second `mission.assigned` event in the outbox).

### 6.2 Scenario AC-2 — collision with different body

**Setup**: client sends `message/send` with `M1` and body B; receives 202.

**Action**: client sends a new request with the same `M1` but body B′ where the JCS canonicalization differs (e.g. different `parts[0].text`, different `metadata`, etc.).

**Expected**:
- HTTP status 422.
- JSON-RPC error with `error.data.code = "IDEMPOTENCY_KEY_REUSED_WITH_DIFFERENT_PAYLOAD"`.
- `error.data.original_request_hash` and `error.data.submitted_request_hash` differ.
- The cache entry for `M1` is unchanged; a follow-up retry with the **original** B still returns the cached response with `Idempotent-Replayed: true`.

### 6.3 Scenario AC-3 — past-TTL replay

**Setup**: client sends `message/send` with `M1` and body B at time T0; entry inserted at T0.

**Action**: at time T1 > T0 + `ttl_seconds`, client sends the same `M1` and body B (or any body).

**Expected**:
- Server treats the request as new.
- Server processes side effects normally.
- A new cache entry is inserted at T1 (overwriting any stale row for `M1`, or, if pruned by the sweep loop, simply inserted fresh).
- Response does NOT include `Idempotent-Replayed: true`.

### 6.4 Scenario AC-4 — cache survives executor restart

**Setup**: client sends `message/send` with `M1` and body B at time T0; receives response R.

**Action**: operator restarts the executor process (or the host reboots). At time T1 < T0 + `ttl_seconds`, client retries `M1` + B.

**Expected**:
- Response is byte-for-byte R, with `Idempotent-Replayed: true`.
- The reference implementation satisfies this by persisting the `idempotency_cache` table in the SQLite-backed TaskStore that backs A2A Task persistence; the same durability guarantee covers Tasks and idempotency entries. Implementations using ephemeral / in-memory caches do NOT conform.

## 7. Security considerations

### 7.1 Replay window vs. message_id collision risk

The 24-hour TTL is a deliberate tradeoff. A shorter window narrows the surface for adversarial replay of a captured `message_id` + body but undermines the practical retry utility (operators with long retry budgets or queued back-pressure may legitimately retry hours later). A longer window inflates the cache and the chance of an attacker reusing a captured `message_id` after the original client has moved on. 24h matches Stripe/OpenAI convention and is the spec-conformant value.

The cache binds `message_id` to a `request_hash`, not just to a key. An attacker who captured `message_id = M1` cannot use it to inject a different payload — that yields the 422 collision response (§6.2). The attacker would need to replay the **identical** request, which is by design the no-op replay (§6.1) and produces no new side effect.

Servers MUST reject `message_id` values that fail basic shape validation (not a UUID-formatted string, or shorter than 128 bits of entropy). Conformance harness scenarios SHOULD include a malformed-`message_id` test that expects a `400 Bad Request` response.

### 7.2 message_id entropy and collision resistance

Clients MUST generate `Message.message_id` from one of:

- **UUIDv7** (RFC 9562 §5.7): time-ordered, 122 bits of entropy after the version and variant bits. Recommended for orchestrators that benefit from time-ordered cache keys.
- **128-bit cryptographically random** value rendered as UUID (effectively UUIDv4 RFC 9562 §5.4): 122 bits of entropy.

At 122 bits of entropy, the probability of collision among 10^9 distinct keys is on the order of 10⁻¹⁹ — negligibly small. Schemes with materially less entropy (incrementing integers, timestamps without random suffix, hostname-prefixed counters with low-bit suffix, etc.) MUST NOT be used: a collision creates either a silent dedup of two unrelated requests (if hashes match — extremely unlikely) or, more dangerously, a 422 that blocks a legitimate request because an unrelated client previously used the same key.

Servers SHOULD log key-format anomalies (e.g. non-UUID `message_id` values) at a level operators can monitor; a sustained anomaly rate is an integration bug.

### 7.3 Payload-hash binding prevents response substitution

Caching `(message_id, request_hash, response)` rather than `(message_id, response)` prevents an attack where an adversary with the ability to inject a request (but not to read the cache) submits a malicious payload under a captured `message_id` hoping the server returns the previously-cached response to a confederate. The hash binding requires the adversary to submit the **identical** canonicalized body to receive the cached response — at which point the operation has no new effect by definition. There is no mechanism by which the cached response can be returned to a payload it was not generated from.

Operators MUST NOT relax this binding (e.g. cache by `message_id` alone). Doing so converts the extension into a vector for response substitution and is non-conforming.

### 7.4 Cache as a side channel

The `Idempotent-Replayed: true` header confirms cache state, which is a (mild) side channel: an attacker who can submit requests can probe whether a given `message_id` is in the cache. This is acceptable because:

- The attacker cannot extract the cached response without already knowing the canonicalized body (per §7.3).
- `message_id` values are 122-bit entropy; blind probing is computationally infeasible.
- Operators concerned about even this signal MAY suppress the header on responses to unauthenticated callers; production deployments authenticate every in-scope call (§3 — required extension), so the side-channel surface is restricted to authenticated callers, who have direct query access to their own request history regardless.

### 7.5 Cache poisoning via failed responses

Failed responses (4xx, 5xx) are cached. A retry returns the original failure. This is intentional: it prevents retry-storms from masking the original error and converging on a different (potentially worse) failure mode. Operators concerned about transient infrastructure failures being cached SHOULD configure the surrounding orchestration (per ADR-007 fail/error split) to issue retries with a **new** `message_id` for infrastructure-class failures, and re-use the same `message_id` only for application-class failures where the cached failure is the correct outcome.

### 7.6 Storage exposure

Cached responses contain the same data as the response wire bytes — including any tokens, identifiers, or payload contents the operation returned. The `idempotency_cache` table inherits the same encryption-at-rest, file-permission, and backup-handling posture as the rest of the executor's persistent store. Operators MUST NOT replicate the cache to less-protected systems for analytics or warm-spare purposes without applying the same controls.

## 8. Backward compatibility

This is a v1 extension; there is no prior version. A future v2 (under URI `…/idempotency/v2`) would be a new URI per ADR-019; v1 and v2 MAY be advertised simultaneously by a server to support clients on either revision.

Within v1, additive changes (new optional `params` keys, new conformance scenarios that do not invalidate previous ones) are permitted and MUST bump the spec `version` field (`1.0.0` → `1.1.0`).

## 9. Reference implementation

- Crate: `agentic-sandbox-executor`
- Module: `extensions::idempotency`
- Storage: shared SQLite database with `TaskStore` (ADR-014); table `idempotency_cache`
- Configuration: env vars `AIWG_IDEMPOTENCY_TTL` (seconds, default 86400), `AIWG_IDEMPOTENCY_MAX_ENTRIES` (default 100000)
- Conformance: covered by the agentic-sandbox conformance harness (ADR-010), scenarios `idempotency-v1-ac1` through `idempotency-v1-ac4`

## 10. Examples

See the sibling `examples/` directory:

- [`examples/replay-hit.http`](examples/replay-hit.http) — AC-1: identical replay returns cached response with `Idempotent-Replayed: true`.
- [`examples/key-collision-422.http`](examples/key-collision-422.http) — AC-2: same `message_id`, different body, 422 response.
- [`examples/post-ttl-new-request.http`](examples/post-ttl-new-request.http) — AC-3: replay after TTL expiry processed as new.

## 11. References

- A2A specification: https://a2a-protocol.org
- A2A extension governance: A2A `docs/topics/extension-and-binding-governance.md`
- RFC 2119 — Key words for use in RFCs
- RFC 8174 — Ambiguity of uppercase vs lowercase in RFC 2119 key words
- RFC 8785 — JSON Canonicalization Scheme (JCS)
- RFC 9562 — Universally Unique IDentifiers (UUIDs)
- IETF draft `draft-ietf-httpapi-idempotency-key-header`
- ADR-008 — `Idempotency-Key` as Dispatch Contract (reframed under A2A)
- ADR-014 — Outbox / event delivery (cache shares storage)
- ADR-019 — Extension URI scheme and governance
- UC-006 — Orchestrator Dispatches Mission to Sandbox via v2 Contract
