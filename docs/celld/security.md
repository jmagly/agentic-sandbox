# Celld integration threat model

Status: approved for bounded POC only. Production remains gated by #754 evidence.

## Trust boundaries and protected assets

Celld's internal operator API is unauthenticated upstream and its object-store credential is root authority for durable cell state. Agentic Sandbox therefore treats the upstream internal listener as a privileged control plane, never as an application endpoint. Protected assets are desired intent, generation/fencing state, effect history, application bundles, peer keys, bucket credentials, operator identity, and incident evidence.

The authorized topology is a single administrative trust domain per fleet. Hostile multi-tenant workloads, public internal listeners, shared bucket-root credentials across fleets, and unqualified artifacts are prohibited.

## Controls

| Threat | Prevent/detect/respond control |
|---|---|
| Public or lateral access to internal API | Private/encrypted overlay, firewall allowlist from management and fleet nodes, distinct public/internal listeners, and no public route. TLS is required off loopback; production terminates mutually authenticated TLS at the fleet proxy. |
| Forged or replayed management request | HMAC-SHA-256 over method, path, body digest, timestamp, nonce, operation ID, and generation; two-minute freshness window; nonce replay cache; constant-time verification. Generation fencing and operation idempotency remain mandatory even after authentication. |
| Bucket credential theft | Dedicated prefix-scoped credential per fleet, broker reference and systemd credential file delivery, mode 0600 validation, 15-minute dual-key rotation, redaction tests, no secret values in manifests or environment diagnostics. |
| Stale owner destroys newer generation | Management is authoritative for observed generation; commands with older or future generations fail; tombstones outlive retry windows; reconciliation never invents a new operation ID for unknown outcomes. |
| Supply-chain substitution | Exact Celld version, commit, archive SHA-256, Worker digest, compatibility date, and adapter version appear in status/manifest. Verify provenance before immutable installation; refuse unknown pairs. |
| Resource exhaustion | Per-bundle CPU, memory, request, storage, and resident-cell ceilings plus fleet RSS/cell ceilings and alarms. Isolates cannot escape to OS process or raw network APIs. |
| Evidence destruction | Structured append-only event history, object-store versioning where available, independent audit retention, synchronized clocks, W3C trace IDs, and read-only evidence export before containment. |

## Secret and key lifecycle

The management signer and callback verifier read `AGENTIC_CELLD_AUTH_KEY_FILE`; there is deliberately no raw-key environment variable. The file must not be group/world accessible and must contain at least 32 bytes. In the disposable fleet, Celld reads the corresponding Worker values from one run-scoped, mode-0600 `CELLD_VARS_FILE` mounted read-only into each node; the value never appears in `wrangler.json`, Docker arguments, or Docker environment values. Both directions sign method, path, body digest, timestamp, nonce, operation ID, and generation. Management additionally requires the callback body, signed headers, and `Idempotency-Key` to name the same effect before the durable ledger can dispatch it. Key IDs are non-secret and appear in audit records.

The qualification Worker client accepts only an exact host-loopback origin,
reads the same protected vars file directly, caps JSON responses at 4 KiB, and
rejects a response that echoes its key, request signature, or nonce. Callers
receive status and bounded JSON only; authentication headers are never returned.

Remote management-to-Celld traffic requires a private CA and a mode-0600 combined PEM client identity. That client disables public root certificates and environment proxies, so the private control request is authenticated only by the configured trust root and sent directly. The pinned Worker API cannot present a client certificate. Its managed callback therefore terminates first at a fixed node-loopback relay; the relay presents the exact client certificate to management without parsing or logging the HMAC-authenticated request. The callback route bypasses operator-role resolution and instead requires the exact CN extracted from a certificate already verified by the management mTLS listener, plus its generation-bound HMAC and durable operation identity; caller-supplied headers cannot supply the certificate identity. The callback CN must remain absent from the mTLS admin allowlist, so the relay cannot acquire authority on any other management route. The relay has no HMAC key, object-store identity, provider credential, public listener, or independent management route. Missing or partial remote mTLS configuration prevents Celld startup, and a missing or wrong callback certificate is rejected before HMAC verification or provider dispatch.

Rotation uses one active signing key and at most one previous verification key. The previous-key ID, credential, valid-from, and valid-until values are an all-or-none set; the IDs must differ and the overlap must be greater than zero and no longer than 15 minutes. Deploy both verification keys, switch signing to the new active key, confirm bidirectional traffic, and remove the previous key after its window. A previous key is accepted only inside its declared interval. Failed canary validation keeps the original active configuration and must not broaden the overlap.

## Incident response

On suspected control-plane or bucket compromise:

1. Block public and cross-fleet routes to the internal listener; preserve node, proxy, management, and object-store logs.
2. Freeze destructive commands while continuing read-only inventory. Record fleet, cell, generation, operation, trace, key ID, artifact digest, and clock evidence.
3. Rotate management authentication, peer, and bucket credentials. Do not delete the old bucket or tombstones.
4. Compare Celld desired state and effect ledgers with management inventory; classify unknown outcomes before any retry.
5. Replace nodes from verified immutable artifacts, restore only from versioned objects, and reconcile generation-by-generation.
6. Resume effects only after stale actors are fenced and security signs the evidence record.

Every exercise must prove evidence export is readable independently of the affected fleet. If it cannot, production approval is revoked.
