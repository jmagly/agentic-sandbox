# ADR-019: agentic-sandbox Extension URI Scheme and Governance

## Status

Accepted (2026-05-09)

## Context

ADR-018 commits to A2A as the base protocol. A2A explicitly supports domain-specific additions via extensions (declared in AgentCard, identified by URI, activated via `A2A-Extensions` HTTP header). Per A2A's extension governance:

- Anyone may author and publish extensions independently.
- Extensions are identified by URI; URI uniqueness across the ecosystem is the implementer's responsibility.
- A2A's official extension repository (`a2aproject` org with `ext-*` repo prefix) is reserved for extensions sponsored by an A2A Maintainer and accepted by TSC vote.
- Versioning by URI: `https://x/ext/v1` → new URI for breaking change.

We need to ship five extensions for v2.0 (`runtime/v1`, `hitl-prompt/v1`, `idempotency/v1`, `multi-tenant/v1`, `pty-extensions/v1`) plus reserve room for future ones. We need to decide where they live, how they're versioned, and the path (if any) toward A2A upstream graduation.

## Decision

**Authoritative URI prefix**: `https://agentic-sandbox.aiwg.io/extensions/<name>/v<major>` (mirrors A2A core's `https://a2a-protocol.org/extensions/` style).

**Repository**: extension specifications live in `roctinam/agentic-sandbox` under `docs/contracts/extensions/<name>/v<major>/spec.md` (and supporting `schemas/`, `examples/` subdirs as needed). Single repo for v2.0; a future split into `roctinam/agentic-sandbox-contracts` is reserved if any extension graduates upstream.

**Naming convention**: short, descriptive lower-case slug. Domain prefix hyphenated (`hitl-prompt`, `multi-tenant`, `pty-extensions`).

**Versioning rules**:
- URI carries major version: `https://agentic-sandbox.aiwg.io/extensions/runtime/v1`.
- Within a major, only additive changes (new optional fields, new methods that don't break existing behavior).
- Breaking change → new URI: `.../runtime/v2`.
- Each extension spec includes a `version` field (e.g. `1.2.0`) for tracking minor additions; URI is the wire-level version.
- Stability tier per A2A convention: `stable | beta | experimental`. Tier moves require spec-version bump.

**Activation**:
- Agents declare supported extensions in AgentCard `capabilities.extensions[]` with URI, description, `required: bool`, optional `params`.
- Clients activate via `A2A-Extensions: <uri1>,<uri2>,...` HTTP header.
- Agent echoes activated extensions in response `A2A-Extensions` header.
- Required extensions: agent rejects request if not activated.

**Spec format**: each extension spec MUST include:
- URI(s) that identify it.
- Schema and meaning of `params` field on the AgentCard `AgentExtension` object.
- Schemas of any new data structures.
- Details of new request-response flows, methods, or state transitions.
- Reference implementation pointer (Rust crate name in our case).
- Conformance test scenarios.
- Security considerations.
- Dependencies on other extensions (if any).

**Initial extension catalog (v2.0)**:

| URI | Purpose | Tier |
|---|---|---|
| `https://agentic-sandbox.aiwg.io/extensions/runtime/v1` | VM/container runtime metadata; `runtime:vm`/`runtime:container` capability flags; loadout config; instance_id | stable |
| `https://agentic-sandbox.aiwg.io/extensions/hitl-prompt/v1` | Structured HITL envelope on `INPUT_REQUIRED` state — prompt_id, response_schema, deadline, allowed_responders | stable |
| `https://agentic-sandbox.aiwg.io/extensions/idempotency/v1` | Server-side dedup keyed on `Message.message_id` for 24h with payload-hash binding | stable |
| `https://agentic-sandbox.aiwg.io/extensions/multi-tenant/v1` | tenant_id metadata + namespace semantics | beta (declared v2.0, enforced v2.2) |
| `https://agentic-sandbox.aiwg.io/extensions/pty-extensions/v1` | Multi-controller roles, Keyframe replay, MembershipChanged events; rides on the `pty-ws/v1` custom binding (ADR-020) | beta |

**Reserved future extensions**:

| URI | Purpose | Status |
|---|---|---|
| `https://agentic-sandbox.aiwg.io/extensions/cost-budget/v1` | Pre-declared cost/token budget per task | reserved (v2.x) |
| `https://agentic-sandbox.aiwg.io/extensions/affinity/v1` | Instance pinning / scheduling hints | reserved (v2.x) |
| `https://agentic-sandbox.aiwg.io/extensions/operator-audit/v1` | Operator-decision audit pointers (orchestration-side: aiwg) | reserved |

**Upstream graduation path**: extensions of broad applicability (`runtime/v1`, `hitl-prompt/v1` candidates) MAY be proposed for A2A experimental-extension status post-v2.0 release. Per A2A governance, requires A2A Maintainer sponsorship and creation under `a2aproject` org with `experimental-ext-*` prefix. Graduation to `ext-*` requires TSC vote with quorum. We do NOT pursue upstream graduation in v2.0; we ship under our URI first, accumulate adoption evidence, then propose.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. In-repo `docs/contracts/extensions/` (chosen)** | Single repo for v2.0; simpler ownership; co-versioning with code | Less obvious for upstream graduation later |
| B. Separate `roctinam/agentic-sandbox-contracts` repo | Mirrors A2A's split; cleaner for upstream graduation | Repo overhead; tooling friction |
| C. Per-extension repo (`roctinam/asb-ext-runtime`, etc.) | Maximum modularity; matches A2A's experimental-ext pattern | Repo proliferation; harder to keep version-coordinated |
| D. Skip our own extensions; everything via A2A core | Smallest spec surface | Cannot express VM/container runtime semantics, HITL structure, idempotency dedup; would force everything into raw `metadata` |

Option B is reserved as a future split if extensions graduate; A starts simpler.

## Consequences

### Positive

- Five extension specs are authored under one URI prefix; consistent governance and discovery.
- Spec format is regular: any extension we author looks the same as any other; adopters' integration burden is uniform.
- Versioning by URI is unambiguous: a new major is a new URI, never reuse.
- Reserved future extensions are visible; doesn't surprise integrators when they appear.
- Upstream graduation path is defined but not committed; we ship first, propose later.

### Negative

- Five specs to author for v2.0 release. Tracked as separate issues.
- We own the URI namespace `agentic-sandbox.aiwg.io/extensions/` — must be served (HTTP 200 with spec content) for permanent identifier discipline. Can host on the same docsite as the rest of `docs.aiwg.io/agentic-sandbox/`.
- Permanent-identifier discipline (per A2A extensions doc): authors are encouraged to use a permanent identifier service (`w3id.org`) to prevent broken links. v2.0: hosted on our docsite; if we change domains in v3.0, we redirect.

### Neutral

- A2A core extensions (e.g. `secure-passport`, `traceability`) are independent; we don't conflict.

## Implementation Notes

- Spec files: `docs/contracts/extensions/{name}/v{n}/spec.md`
- Reference Rust types: re-exported from `agentic-sandbox-executor` crate's `extensions::{name}` module
- AgentCard publication includes the activated extensions array; per ADR-022 each instance gets its own AgentCard with the relevant extensions for that instance's runtime kind.
- Conformance harness (ADR-010) loads extension specs and generates per-extension test scenarios.

## Related

- ADR-018 (A2A as base protocol)
- ADR-020 (PTY custom binding — companion to `pty-extensions/v1`)
- ADR-022 (three-surface architecture — defines where AgentCard lives)
- A2A extensions governance: `/home/roctinam/dev/A2A/docs/topics/extension-and-binding-governance.md`
- Gap matrix: `.aiwg/working/issue-planner/a2a-gap-matrix.md` (Tier 2)
