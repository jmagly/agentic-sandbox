# OWASP ASVS and Top 10 API Security Profile

Date: 2026-06-22

Scope: Agentic Sandbox local-first management, dashboard, WebSocket/PTTY, and
agent control surfaces through `v2026.6.28`.

This profile uses OWASP ASVS as a verification checklist and OWASP Top 10 as a
risk closure pass. It is not an OWASP certification or external assessment.

## Status Terms

| Status | Meaning |
| --- | --- |
| Covered | Current implementation evidence and tests exist for the target scope. |
| Partial | Controls exist, but target-release evidence or coverage is incomplete. |
| Gap | Control is required for the target scope and remains open. |
| Not applicable | Requirement does not apply to the current local-first surface. |

## Target Profile

| Surface | ASVS target | Status | Boundary |
| --- | --- | --- | --- |
| HTTP management API (`8122`) | ASVS Level 1 baseline for local operator deployments; Level 2 for authenticated remote/operator deployments. | Partial | Local-host operation is the current default claim. Remote use requires bearer, mTLS, Unix peer credentials, trusted tunnel, or reverse proxy. |
| WebSocket telemetry (`8121`) | ASVS Level 1 baseline for local operator telemetry; Level 2 for remote/PTY attach. | Partial | Legacy plaintext WS is local-only. Production PTY attach uses the authenticated `pty-ws/v1`/WSS contract. |
| gRPC agent control plane (`8120`) | ASVS Level 2 equivalent for service-to-service control traffic. | Partial | Secure paths use UDS, vsock, or mTLS identity; release-specific negative transport verification remains tracked by #507. |
| Dashboard UI | ASVS Level 1 baseline plus CSP/DOM-sink hardening before any remote-admin claim. | Gap | UI escaping helpers exist, but CSP and full user-controlled sink regression are tracked by #551. |
| AIWG executor dispatch route | ASVS Level 2 for bearer-authenticated dispatch. | Covered | `POST /api/v1/sessions/{id}/dispatch` requires `Authorization: Bearer <token>` with constant-time comparison. |

## Surface Control Matrix

| Surface | Authentication | Authorization | Input validation | Error handling | Logging / audit | Security headers / browser controls | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| HTTP management API | Operator auth supports bearer, mTLS, and Unix peer credentials when configured. Local mode can run with auth disabled. | Admin-only extractors guard destructive routes; route-by-route evidence is incomplete. | JSON/OpenAPI schemas and handler validation exist for v2 admin, credentials, startup profiles, and contracts. | Structured error envelopes are documented for v2 admin; v1 has mixed legacy JSON errors. | Audit modules record auth and authorization outcomes; route coverage varies. | No complete HTTP header hardening matrix is published. | Partial |
| WebSocket telemetry (`8121`) | Local-host access only by current claim. No general remote WS auth claim. | Legacy event/output paths scope command output, but old telemetry remains local-only. | JSON frame parsing and command scoping exist; full malformed-frame matrix is incomplete. | Connection close/error behavior exists; evidence is not consolidated. | Output authorization and session events have partial coverage. | Browser-side protections depend on dashboard hardening. | Partial |
| `pty-ws/v1` attach | Spec requires bearer auth on upgrade for production, with optional hash-only attach token map. | `pty:observe` and `pty:control` scopes are specified; observers must not input/resize. | Frame schemas define envelope and PTY extension payloads. | Binding specifies `4401`, `4403`, and structured errors. | Session membership, replay, and role frames are defined. | Uses WSS in production. | Partial |
| gRPC agent control | Transport identity required; legacy shared secret is retired. | Agent identity binds to transport peer identity and instance id. | Protobuf schema and metadata validation exist. | Unauthenticated and mismatch cases return gRPC unauthenticated errors. | Registry records authenticated transport posture. | Not a browser surface. | Partial |
| Dashboard UI | Inherits local HTTP boundary; no standalone user auth model. | UI exposes operator controls; remote multi-user admin is not claimed. | Some values use `textContent`, escaping helpers, and safe markdown rendering. | UI displays API errors; no XSS-focused error rendering matrix. | UI displays logs/events that can include attacker-controlled text. | CSP and broad DOM-sink regression remain open. | Gap |
| AIWG executor dispatch | Bearer token required. | Token is scoped to registered executor dispatch. | Request body is validated before dispatch. | `401`, `404`, `503`, and `500` behavior is documented. | Mission assignment/failure events are emitted. | Not a browser surface. | Covered |

## HTTP And WebSocket Auth Evidence Matrix

| Auth mode / route family | Expected behavior | Evidence | ASVS status |
| --- | --- | --- | --- |
| Auth disabled, local compatibility mode | Requests pass through for local/trusted-network deployments; this mode must not be marketed as remote-admin auth. | `management/src/http/operator_auth.rs::asvs_operator_auth_decision_matrix_covers_configured_modes` checks `PassThrough`. | Partial |
| Bearer auth enabled, missing or invalid token | Sensitive routes reject before handler state is exposed. | `management/src/http/operator_auth.rs::asvs_operator_auth_decision_matrix_covers_configured_modes` checks `Unauthorized`; middleware emits `401` with `WWW-Authenticate: Bearer`. | Partial |
| Bearer auth enabled, admin token | Admin token resolves to `OperatorRole::Admin`; admin-only extractors allow the request. | `asvs_operator_auth_decision_matrix_covers_configured_modes`; `require_admin_enforces_admin_role_when_auth_resolved`. | Partial |
| Bearer auth enabled, operator token | Operator token resolves to `OperatorRole::Operator`; admin-only extractors return `403`. | `asvs_operator_auth_decision_matrix_covers_configured_modes`; `require_admin_enforces_admin_role_when_auth_resolved`. | Partial |
| mTLS admin allowlist | Allowed client certificate CN grants admin; denied CN returns `403` and does not fall through to bearer. | `asvs_operator_auth_decision_matrix_covers_configured_modes`; `mtls_does_not_fall_through_to_bearer`. | Partial |
| Unix peer credentials | Allowed UID grants admin; denied UID returns `403`; unset allowlist preserves UDS filesystem-ACL compatibility. | `asvs_operator_auth_decision_matrix_covers_configured_modes`; `unix_peer_creds_config_back_compat_grants_any_uid`. | Partial |
| Metadata and health exceptions | Health, readiness, bootstrap-enrollment consume, AgentCard, JWKS, and card metadata bypass auth by design; deeper/sensitive paths do not. | `metadata_paths_bypass_auth`. | Covered |
| PTY attach auth scopes | Bearer admin maps to PTY admin scope; bearer operator maps to PTY control scope; unknown token has no attach scope. | `pty_attach_authorizer_maps_operator_roles_to_attach_scopes`; `docs/contracts/bindings/pty-ws/v1/spec.md`. | Partial |
| AIWG dispatch route | Dispatch route requires the AIWG executor bearer token and returns `401` on invalid token. | `management/src/http/dispatch.rs`; `docs/API.md` dispatch route. | Covered / partial |
| Legacy WebSocket telemetry | Local-only by current claim; remote use requires authenticated `pty-ws/v1`/WSS or trusted tunnel. | `docs/API.md`; [attack surface inventory](attack-surface.md). | Partial |
| Non-loopback/plaintext dev and Docker reachability | Unsafe plaintext bind guidance and Docker-reachable bootstrap failures must be explicit and fail early in dev/runtime profiles. | Tracked as implementation follow-ups #549 and #550; [attack surface inventory](attack-surface.md) documents the launch boundary. | Partial |

## ASVS Category Mapping

| ASVS category | Project interpretation | Current evidence | Status | Follow-up |
| --- | --- | --- | --- | --- |
| V1 Architecture, Design, Threat Modeling | Local-first trust boundary, three-surface model, STRIDE threat models, attack surface inventory. | [attack surface inventory](attack-surface.md), `.aiwg/security/threat-model.md`, `.aiwg/security/agent-transport-threat-model.md`, `.aiwg/architecture/adr/ADR-022-three-surface-architecture.md`. | Covered | Keep current with release changes. |
| V2 Authentication | Operator bearer/mTLS/UDS auth; gRPC UDS/vsock/mTLS identity; AIWG dispatch bearer token. | `management/src/http/operator_auth.rs`, `management/src/grpc.rs`, `docs/API.md`, [agent transport CA backends](agent-transport-ca-backends.md), HTTP/WS auth evidence matrix above. | Partial | #507 |
| V3 Session Management | PTY attach roles, replay, observers/controllers, dispatch lifecycle. | `docs/contracts/bindings/pty-ws/v1/spec.md`, `docs/contracts/extensions/pty-extensions/v1/spec.md`, `management/src/session/registry.rs`, HTTP/WS auth evidence matrix above. | Partial | Keep expanding route-level session tests as surfaces change. |
| V4 Access Control | Admin-only HTTP extractors, PTY observe/control distinction, SSH gateway authorization. | `management/src/http/operator_auth.rs`, `management/src/http/ssh_gateway.rs`, `docs/API.md`, HTTP/WS auth evidence matrix above. | Partial | Keep expanding route-level access-control tests as new admin APIs land. |
| V5 Validation, Sanitization, Encoding | JSON schemas, OpenAPI contracts, command adapter allowlists, UI escaping helpers. | `docs/contracts/`, `docs/contracts/admin-api.openapi.yaml`, `management/src/agent_message_dispatch.rs`, `management/ui/app.js`. | Partial | #551 |
| V6 Stored Cryptography | No user password store; token hashing and TLS/key material are scoped to operator and agent control. | `management/src/http/operator_auth.rs`, `management/src/audit/secrets_rotation.rs`, `.codex/rules/no-adhoc-kdf.md`. | Partial | #507 |
| V7 Error Handling and Logging | Structured admin errors, audit event types, credential redaction patterns. | `docs/contracts/admin-api/error-envelope.schema.json`, `management/src/audit/`, `management/src/session/redaction.rs`. | Partial | #518 |
| V8 Data Protection | Credential values are write-only/metadata-first where implemented; transcript and logs are sensitive. | [attack surface inventory](attack-surface.md), `.aiwg/security/credential-posture-2026-06-19.md`, `.aiwg/security/attack-informed-test-catalog.md`. | Partial | #516, #517, #518 |
| V9 Communications | UDS/vsock/mTLS agent control; WSS required for production PTY binding. | `.aiwg/security/agent-transport-threat-model.md`, `docs/contracts/bindings/pty-ws/v1/spec.md`. | Partial | #507 |
| V10 Malicious Code | Supply-chain and release verification are handled outside app/API profile. | [release verification](../releases/verification.md), [standards alignment](standards-alignment.md). | Partial | #509 |
| V11 Business Logic | Session dispatch, startup profiles, credential refs, and runtime provisioning need workflow-specific checks. | `docs/API.md`, `docs/contracts/`, `.aiwg/security/attack-informed-test-catalog.md`. | Partial | #518 |
| V12 Files and Resources | Storage APIs, agentshare, upload/download, quotas, and runtime resource limits. | `docs/security/resource-quota-design.md`, `management/tests/e2e_resource_limits.rs`. | Partial | #510 follow-on test expansion as needed. |
| V13 API and Web Service | OpenAPI/admin contracts, WebSocket binding, gRPC control plane, executor dispatch. | `docs/contracts/`, `docs/API.md`, `management/src/grpc.rs`, HTTP/WS auth evidence matrix above. | Partial | Keep route-level API tests aligned with new surfaces. |
| V14 Configuration | Local defaults, unsafe non-loopback guidance, release and runtime configuration. | [attack surface inventory](attack-surface.md), `docs/API.md`, `management/src/config.rs`. | Partial | #549, #550 |

## OWASP Top 10 Closure Pass

| OWASP risk | Current status | Evidence / decision |
| --- | --- | --- |
| A01 Broken Access Control | Partial | Admin extractors, PTY observer/control rules, SSH gateway authorization, and HTTP/WS operator-auth matrix tests exist. Continue adding route-level checks as new admin APIs land. |
| A02 Cryptographic Failures | Partial | Agent transport identity uses UDS/vsock/mTLS. Release-specific AC-1..AC-8 verification remains #507; credential proxy and leak harness remain #516/#518. |
| A03 Injection | Partial | OpenAPI/JSON schemas and command adapter allowlists exist; dashboard DOM injection/CSP hardening remains #551. |
| A04 Insecure Design | Partial | STRIDE, ADRs, attack surface, standards matrix, and attack-informed catalog are published. Runtime credential and proxy bypass controls remain open. |
| A05 Security Misconfiguration | Partial | Local-first defaults and unsafe remote exposure warnings are documented. Non-loopback/dev fail-closed guidance is tracked by #549/#550. |
| A06 Vulnerable and Outdated Components | Partial | Release verification docs exist; base image/qcow2/loadout provenance closure remains #509. |
| A07 Identification and Authentication Failures | Partial | AIWG dispatch bearer, operator-auth mechanisms, PTY attach auth mapping, and HTTP/WS enforcement matrix tests exist. Transport verification remains #507. |
| A08 Software and Data Integrity Failures | Partial | Checksums/SBOM/signature verification docs exist. Base image and provisioning provenance remain #509. |
| A09 Security Logging and Monitoring Failures | Partial | Audit event types and redaction helpers exist. Route-by-route auth/audit should continue with new surfaces; fake-secret log leakage evidence remains #518. |
| A10 Server-Side Request Forgery | Gap | Credential proxy and egress bypass tests are not implemented yet; tracked by #516/#518. |

## Current P1 Implementation Links

| Gap | Tracker |
| --- | --- |
| Dashboard CSP and user-controlled DOM sink regression. | #551 |
| Transport release verification. | #507 |
| Credential proxy implementation and leakage/bypass harness. | #516, #517, #518 |
| Base image, qcow2, cloud-init, and loadout provenance closure. | #509 |
| Docker reachability and plaintext bind guidance. | #549, #550 |

## Claim Guidance

Safe current wording:

- "Local-first API and dashboard surfaces with optional operator auth for
  bearer, mTLS, and Unix peer-credential deployments."
- "Production PTY attach is specified for authenticated WSS with observe/control
  roles; legacy plaintext WebSocket telemetry is local-only."
- "Agent control identity is designed around UDS, vsock, or mTLS transport
  identity, not reusable shared secrets."

Avoid until follow-ups close:

- "The dashboard is CSP-hardened or fully XSS-audited."
- "All HTTP/WebSocket management APIs are remotely authenticated by default."
- "All OWASP ASVS Level 2 controls are verified."
