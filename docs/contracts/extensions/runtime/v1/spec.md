# A2A Extension: `runtime/v1`

> **Stability tier**: `stable`
> **Spec version**: `1.1.0`
> **Extension URI**: `https://agentic-sandbox.aiwg.io/extensions/runtime/v1`
> **Status**: Published (v2.0)

This extension declares the **execution substrate** (VM, container, or host) that
backs an agentic-sandbox A2A agent and surfaces the **runtime instance identity**
on each Task. It exists because A2A core has no opinion on whether an agent runs
in a VM, container, or bare process — but consumers of agentic-sandbox routinely
need that metadata for routing, auditing, and lifecycle management.

The keywords **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, **MAY**, and
**REQUIRED** in this document are to be interpreted as described in
[RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) and
[RFC 8174](https://www.rfc-editor.org/rfc/rfc8174).

---

## 1. URI

The canonical extension identifier is:

```
https://agentic-sandbox.aiwg.io/extensions/runtime/v1
```

This URI MUST be used verbatim in:

- `AgentCard.capabilities.extensions[].uri`
- the HTTP request header `A2A-Extensions: <uri>` for activation
- the HTTP response header `A2A-Extensions: <uri>` for echo / confirmation

Permanent identifier discipline follows ADR-019. The URI resolves (HTTP 200) to
this specification.

## 2. AgentCard `params` schema

When an AgentCard declares this extension under `capabilities.extensions[]`, the
`params` object on that entry MUST conform to
[`params.schema.json`](./params.schema.json).

```json
{
  "uri": "https://agentic-sandbox.aiwg.io/extensions/runtime/v1",
  "description": "VM/container/host runtime metadata for this instance.",
  "required": true,
  "params": {
    "runtime": "vm",
    "loadout": "agentic-dev",
    "image_ref": "qcow2://images/ubuntu-24.04-agentic-dev@sha256:…",
    "instance_id": "550e8400-e29b-41d4-a716-446655440000"
  }
}
```

### 2.1 Field semantics

| Field | Type | Required | Meaning |
|---|---|---|---|
| `runtime` | enum: `"vm" \| "container" \| "host"` | yes | The runtime kind backing this instance. |
| `loadout` | string | yes | Name of the agentic-sandbox loadout (profile + agent toolchain) provisioned. Maps 1:1 to a `loadout.yaml` known to the management server. |
| `image_ref` | string | no | Reference to the underlying image. For `vm`: qcow2 URL with digest. For `container`: OCI image reference (`registry/name@sha256:…`). For `host`, this field is absent because no image boundary exists. When omitted, the consumer MUST treat the image as opaque. |
| `instance_id` | string (UUID v4) | yes | Stable identifier of the runtime instance. Constant for the lifetime of the underlying VM or container. |

For `runtime = "host"`, the runtime instance is a supervisor-owned local
process tree, not an unmanaged child of the admin HTTP handler. The supervisor
MUST own liveness, cleanup, PTY/session attachment, reattach, and multi-agent
coordination on that host. Consumers MUST treat the isolation level as full
host access unless out-of-band OS policy says otherwise.

`additionalProperties` is `false`. `runtime = "host"` was added in spec
version `1.1.0` as an additive runtime kind under the same URI. Consumers MUST
treat unknown future runtime values as opaque for display/audit and MUST NOT
fail closed purely because a new runtime kind appears. Narrowing, renaming, or
repurposing fields still requires a new extension URI (`runtime/v2`) per
ADR-019.

### 2.2 Activation

This extension MUST be declared with `required: true` on every AgentCard published
by an agentic-sandbox instance (ADR-022 §Per-Instance A2A surface). Clients
activate it by sending:

```http
A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/runtime/v1
```

If a client omits the header on a request to an agentic-sandbox AgentCard that
declares the extension as required, the agent MUST reject the request with HTTP
`400` and an A2A error body indicating `EXTENSION_REQUIRED`.

The agent MUST echo activated extensions in its response `A2A-Extensions` header.

## 3. Task metadata additions

When this extension is activated, every `Task` object returned or streamed by the
agent MUST include the following entries in `Task.metadata`, conforming to
[`task-metadata.schema.json`](./task-metadata.schema.json):

| Key | Type | Required | Meaning |
|---|---|---|---|
| `runtime.instance_id` | string (UUID v4) | yes | MUST equal the `instance_id` from the AgentCard `params` of the agent that produced the Task. |
| `runtime.kind` | enum: `"vm" \| "container" \| "host"` | yes | MUST equal the AgentCard `params.runtime`. |
| `runtime.host` | string | no | Opaque identifier of the host machine (libvirt host, k8s node, etc.). Disclosed only when policy permits (see §7). |

These keys MUST NOT collide with other extension namespaces; the dotted prefix
`runtime.` is reserved by this extension.

## 4. Data structures

This extension introduces no new top-level message types. All data is carried in
the existing `AgentExtension.params` and `Task.metadata` structures defined by
A2A core.

## 5. Request / response flows

This extension is **data-only**. It defines no new request methods, no new state
transitions, and no new streaming events. It MUST NOT alter the A2A Task lifecycle.

Activation simply causes the agent to populate the `Task.metadata` keys defined in
§3 on every Task object — including Tasks returned by `GetTask`, `SendMessage`,
`SendStreamingMessage` events (initial `Task` snapshot), `SubscribeToTask`, and
`ListTasks`.

## 6. Reference implementation

The reference implementation lives in the `agentic-sandbox-executor` Rust crate:

- Module: `agentic_sandbox_executor::extensions::runtime`
- Types: `RuntimeParams`, `RuntimeKind`, `RuntimeMetadata`
- Validation: `RuntimeParams::validate()` enforces the JSON Schema in
  [`params.schema.json`](./params.schema.json).
- AgentCard publication: the management server's per-instance AgentCard publisher
  attaches `RuntimeParams` automatically when minting an AgentCard for a
  spawned VM or container.

See `docs/contracts/extensions/runtime/v1/examples/` for canonical examples.

## 7. Conformance scenarios

A conforming agent MUST pass the following scenarios. Scenarios are referenced by
the conformance harness (ADR-010).

1. **RUNTIME-CONF-001 — AgentCard declares extension** (UC: discover-agent).
   GIVEN an agentic-sandbox instance is provisioned and running,
   WHEN a client fetches `/.well-known/agent-card.json`,
   THEN the response MUST include exactly one entry in
   `capabilities.extensions[]` whose `uri` equals the canonical extension URI,
   AND the entry MUST have `required: true`,
   AND `params` MUST validate against `params.schema.json`.

2. **RUNTIME-CONF-002 — Activation echoed on response** (UC: invoke-agent).
   GIVEN an agentic-sandbox instance with the extension declared,
   WHEN a client sends a request with header
   `A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/runtime/v1`,
   THEN the response MUST include the same URI in its `A2A-Extensions`
   response header.

3. **RUNTIME-CONF-003 — Required extension enforcement** (UC: invoke-agent).
   GIVEN an agentic-sandbox instance with the extension declared as required,
   WHEN a client sends a request **without** the activation header,
   THEN the agent MUST reject the request with HTTP `400` and an A2A error
   body indicating `EXTENSION_REQUIRED` referencing the canonical URI.

4. **RUNTIME-CONF-004 — Task metadata populated** (UC: get-task, subscribe-task).
   GIVEN a Task created by an agentic-sandbox instance with the extension
   activated,
   WHEN the client calls `GetTask` or receives the Task via
   `SubscribeToTask`,
   THEN `Task.metadata["runtime.instance_id"]` MUST equal the AgentCard
   `params.instance_id`,
   AND `Task.metadata["runtime.kind"]` MUST equal the AgentCard
   `params.runtime`,
   AND if present, `Task.metadata["runtime.host"]` MUST be a non-empty
   string.

5. **RUNTIME-CONF-005 — Image reference integrity**.
   GIVEN an AgentCard whose `params.image_ref` is present,
   WHEN the value declares a `vm` runtime,
   THEN it MUST be a `qcow2://` URL containing an `@sha256:` digest fragment,
   AND when it declares a `container` runtime, it MUST be a valid OCI image
   reference of form `registry/name@sha256:<hex>` or `registry/name:tag`,
   AND when it declares a `host` runtime, `image_ref` SHOULD be absent.

6. **RUNTIME-CONF-006 — instance_id stability**.
   GIVEN an agentic-sandbox instance,
   WHEN the AgentCard is fetched repeatedly across the instance lifetime,
   THEN `params.instance_id` MUST be byte-identical on every fetch
   (no rotation while the underlying VM or container exists).

## 8. Security considerations

### 8.1 `instance_id` disclosure

`instance_id` is a non-sensitive UUID by design — it identifies a runtime
instance, not a tenant or a user. It MAY be logged and forwarded freely.
Implementations MUST NOT derive `instance_id` from secret material; UUID v4
(random) is REQUIRED. Derivation from secrets would create a side channel.

### 8.2 `host` identifier disclosure trade-off

`runtime.host` reveals the physical or logical host backing an instance. This
is useful for auditing, co-tenancy debugging, and noisy-neighbor diagnosis,
but it leaks fleet topology to clients.

Implementations MUST gate disclosure behind an operator-controlled policy.
Recommended defaults:

- **Closed fleets** (single-tenant, internal): emit `runtime.host` freely.
- **Open / multi-tenant fleets** (`multi-tenant/v1` activated): omit
  `runtime.host` unless the requesting principal has the `fleet:read-host`
  scope.

When omitted, the field MUST simply be absent — implementations MUST NOT emit
a placeholder string (e.g. `"redacted"`), as that would itself confirm the
host's existence.

### 8.3 `loadout` name as capability hint

The `loadout` name is a coarse capability hint. An attacker who learns
`loadout: "security-audit"` learns the agent has security-audit tooling
installed. This is generally low-sensitivity — loadout names are part of the
public agent catalog — but operators concerned about reconnaissance value
SHOULD use neutral loadout names (`profile-a`, `profile-b`) rather than
descriptive ones (`bitcoin-miner`, `pii-scrubber`).

`loadout` MUST NOT carry secrets, tokens, or per-tenant identifiers; those
belong in `multi-tenant/v1` metadata or out-of-band configuration.

### 8.4 Host runtime isolation

`runtime = "host"` is the least-isolated runtime tier. It means the instance
runs directly on the operator's host without a VM or container boundary, so it
has the same filesystem, process, credential, and network reachability as the
launching service account unless separately constrained by the host operating
system.

Implementations MUST make host selection explicit. They MUST NOT silently
fallback from `host` to `vm` or `container`, and UIs SHOULD display host as
"full host access" or equivalent. Durable host execution SHOULD be mediated by a
host-side supervisor or service so controller restarts can reattach to existing
process/session state instead of orphaning local shells.

### 8.4 `image_ref` and supply chain

When `image_ref` is published, it SHOULD include a content digest
(`@sha256:…`). Tag-only references (`registry/name:latest`) are PERMITTED but
SHOULD be avoided in production AgentCards because they prevent clients from
reasoning about image identity over time.

### 8.5 No cross-extension leakage

This extension's metadata MUST NOT be used to authenticate or authorize
requests. Authentication is governed by A2A core `securitySchemes`.
`instance_id` is identity-of-runtime, not identity-of-principal.

## 9. Dependencies on other extensions

None. This extension is self-contained and MAY be activated independently.

It is **complementary** to `multi-tenant/v1` (which adds `tenant_id`) and
`idempotency/v1` (which adds dedup metadata); when those extensions are also
activated, their metadata appears alongside `runtime.*` keys in `Task.metadata`
without conflict.

## 10. Versioning

Per ADR-019:

- This URI is permanent. It MUST resolve to this specification or a backward-
  compatible successor.
- Within `v1`, only **additive** changes are allowed (new optional fields).
  Additive changes bump the spec version (e.g. `1.1.0`) but the URI stays
  `…/runtime/v1`.
- Breaking changes (removing fields, changing types, narrowing enums,
  re-purposing keys) require a new URI: `…/runtime/v2`.
- Tier moves (`stable` → `deprecated`, etc.) require a spec version bump and
  MUST be announced ≥6 months ahead of any compatibility break.

## 11. References

- ADR-018 — A2A as base protocol
- ADR-019 — Extension URI scheme and governance
- ADR-022 — Three-surface architecture
- A2A v1.0.0 specification — `https://a2a-protocol.org/`
- RFC 2119 / RFC 8174 — Requirement-level keywords
- RFC 4122 — UUID
