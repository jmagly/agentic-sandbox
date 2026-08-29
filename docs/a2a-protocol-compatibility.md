# A2A protocol compatibility

Agentic Sandbox serves A2A 0.3 and A2A 1.0 concurrently. Version selection is
per request; upgrading the server does not force existing 0.3 clients to
change their wire model.

Three independent version namespaces appear in this repository:

- **A2A 0.3 / A2A 1.0** are upstream interaction-protocol versions selected
  with `A2A-Version`.
- **agentic-sandbox v2** is the product/API generation.
- URI suffixes such as `pty-ws/v1`, `runtime/v1`, and `executor.v1` version
  local bindings, extensions, or integration contracts. They do not select an
  upstream A2A protocol version.

## Discovery and selection

The signed AgentCard at
`/agents/{instance_id}/.well-known/agent-card.json` advertises two truthful
HTTP+JSON tuples:

| A2A version | Base URL | Selection |
|---|---|---|
| 1.0 | `/agents/{instance_id}` | Send `A2A-Version: 1.0` |
| 0.3 | `/agents/{instance_id}/v1` | Omit the header, send an empty value, or send `A2A-Version: 0.3` |

The `/v1` path component is a historical Agentic Sandbox compatibility alias;
it does not mean A2A 1.0. A request for 1.0 on that alias fails closed. Values
must use exact `Major.Minor` form, so `1`, `1.0.0`, and `0.3.0` are rejected.
Unsupported or malformed values return HTTP 400 with the A2A
`VersionNotSupportedError` semantics in a `google.rpc.Status` JSON envelope.

Every negotiated response includes `A2A-Version` and
`Vary: A2A-Version, Accept`. Counters use only the bounded labels `0.3`, `1.0`,
and `rejected`; logs record the selected bounded version without using caller
values as metric labels. Idempotency digests include both the decoded content
and negotiated protocol version, preventing a cached 0.3 representation from
being replayed as 1.0.

## HTTP+JSON routes and media types

A2A 1.0 uses the unprefixed routes:

| Operation | A2A 1.0 path |
|---|---|
| Send message | `POST /agents/{id}/message:send` |
| Stream message | `POST /agents/{id}/message:stream` |
| Get/list task | `GET /agents/{id}/tasks/{tid}` / `GET /agents/{id}/tasks` |
| Cancel task | `POST /agents/{id}/tasks/{tid}:cancel` |
| Subscribe | `POST /agents/{id}/tasks/{tid}:subscribe` |

The v1.0.1 media-type correction recommends `application/a2a+json`. The
adapter accepts that type and returns it when the request `Content-Type` or
`Accept` explicitly selects it. It also accepts and mirrors
`application/json` for v1.0 clients and the upstream TCK. Other request media
types receive HTTP 415.

The 0.3 adapter retains `/v1/...`, plural `/messages:*`, slash-form `/cancel`
and `/subscribe` aliases, legacy `application/json`, RFC 7807 project errors,
lowercase task states, lowercase roles, and `kind`-discriminated parts.

## Wire-model boundary

The persistence model is version-neutral. The protocol boundary validates and
converts wire objects before handlers run and converts results after handlers
finish. A2A 1.0 uses `ROLE_*`, `TASK_STATE_*`, member-discriminated Parts
(`text`, `raw`, `url`, or `data`), `mediaType`, ProtoJSON base64 bytes, and
`google.rpc.Status` errors. Mixed legacy/v1 parts, multiple oneof members,
unknown roles, missing identifiers, empty parts, and invalid base64 fail before
task persistence. Unrecognized ProtoJSON fields are ignored for forward
compatibility.

Extension and Flow graph data remains under standard `metadata` maps and is
round-tripped without becoming a standard protocol field. The fixture at
`management/agentic-sandbox-executor/tests/fixtures/a2a-v1.0.1-message.json`
covers text, raw, URL, structured data, metadata, and extensions.

## Migration policy

Clients may migrate one interface at a time:

1. Discover and verify the signed AgentCard.
2. Select the advertised 1.0 HTTP+JSON tuple.
3. Send `A2A-Version: 1.0` and 1.0 ProtoJSON; do not rely on fallback.
4. Keep the 0.3 adapter configured as an explicit rollback path during the
   migration window.

No 0.3 removal date is implied by the local `/v1` names. Deprecation requires
a separate ADR, published operator notice, usage evidence, and a release gate.

## Retained conformance evidence

The `Conformance` workflow runs the two protocol generations independently:

- the AIWG `agentic-sandbox-conformance` harness targets the headerless A2A
  0.3 compatibility interface;
- the upstream `a2aproject/a2a-tck` checkout at commit
  `5996b79f9cefa6fc390980e383e358a66fb9e49e` targets negotiated A2A 1.0
  HTTP+JSON and retains its HTML, JUnit, and compatibility reports.

Local qualification on 2026-08-29 used that exact TCK commit and its declared
v1.0.0 suite. The full HTTP+JSON run completed with 94 passed, 171 skipped
(unselected transports or unsupported optional capabilities), zero failures,
and a 100% compatibility report. The MUST-only run completed with 82 passed,
153 skipped, and zero failures. The official A2A Inspector at commit
`8aa064639af106ff771d60428ef6d460f5454743` fetched the negotiated signed
Agent Card and returned an empty `validation_errors` array. Its observed card
contained the 1.0 and 0.3 HTTP+JSON tuples documented above.
