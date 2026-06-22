# Credential posture decision for launch

Date: 2026-06-19

Scope: ADR-028 credential non-exposure posture, proxy-vs-lease delivery
decision, and launch claim language for Gitea issue #506.

## Decision

The launch posture is **qualified non-exposure**, not an absolute "secrets never
enter workloads" claim.

The current architecture has credible controls for metadata-only credential
references and write-only credential APIs. It does not yet prove that every
provider credential can stay outside every container or VM, because some
provider CLIs and systems require a local file or environment variable inside
the workload.

The correct model is layered delivery:

1. Prefer a credential proxy backend where the protocol can be mediated.
2. Materialize a short-lived lease into a runtime-scoped file when the workload
   requires local secret material.
3. Use final-child environment injection only when the tool has no file or proxy
   option.
4. Treat manual/interactive auth as an explicit policy exception, not as part of
   the non-exposure claim.

## Current evidence

| Area | Evidence | Posture |
| --- | --- | --- |
| ADR baseline | `.aiwg/architecture/adr/ADR-028-workload-credential-leases-and-startup-profiles.md` defines machine identity, workload credential leases, startup profiles, and proxy as one delivery backend. | Proposed architecture baseline. |
| Superseded proxy-first design | `.aiwg/architecture/adr/ADR-002-credential-proxy.md` is superseded for provider/session authorization but remains useful as a delivery backend pattern. | Proxy is a backend, not the whole credential model. |
| Credential metadata API | `management/src/http/credentials.rs` exposes credential metadata and lease APIs; tests assert API responses do not return write-only values. | Implemented core behavior. |
| Credential broker | `management/src/credentials.rs` models metadata and leases. | Implemented core behavior, still needs persistent backend and delivery hardening. |
| Startup profiles | `management/src/startup_profiles.rs` stores `credential_refs` by id, provider, allowed use, and target hint. | Implemented metadata profile model. |
| Agent contract | `agent-rs/src/credentials.rs` initializes a credential reference contract, enforces runtime dir mode `0700`, and rejects inline `value` fields with `deny_unknown_fields`. | Implemented metadata-only agent-side contract. |
| Loadout generation | `images/qemu/loadouts/tests/test_generate_from_manifest.sh` checks that credential refs write metadata policy and reject inline secret-like values. | Implemented generation tests. |
| Provider wrappers | `images/common/automation-control/*` and `images/agent/claude/entrypoint.sh` discover credential files in a runtime credential directory. | File lease materialization path exists. |
| Transport bootstrap | Agent shared secrets are retired; one-time bootstrap enrollment writes mTLS material and scrubs token env file entries. | Machine identity plane substantially improved. |

## What can be claimed now

The following language is supportable for launch, assuming release verification
confirms the cited tests and docs are current:

- Credential records, startup profiles, and agent credential reference policies
  are metadata-first.
- Credential API responses must not return provider token values.
- Provider credentials are referenced by id, provider, allowed use, and target
  hint rather than being stored as durable environment blobs.
- Agent workloads can receive session-scoped credential leases through runtime
  credential directories.
- Bootstrap agent identity no longer relies on the retired shared
  `AGENT_SECRET` path.

## What must not be claimed yet

Do not claim:

- Provider secrets never enter containers or VMs.
- Every credential can be delivered through a proxy.
- Credential lease files are always destroyed on crash paths.
- The broker has a complete persistent encrypted backend, policy engine,
  audit trail, and proxy adapter set.
- Startup profiles are a complete substitute for provider-specific login or
  device-code flows.

## Proxy delivery scope

A proxy works best when the workload can be pointed at a host-controlled
endpoint and the upstream protocol can be authenticated on behalf of the
workload without exposing the upstream secret.

Good proxy candidates:

| Protocol / system | Proxy fit | Notes |
| --- | --- | --- |
| HTTP and REST APIs | High | Workload can call local proxy; proxy injects upstream bearer/API credentials and enforces route allowlists. |
| Git HTTPS | High | Credential helper or remote URL can target proxy; proxy can enforce repo/org scope. |
| Container registries | High | Registry auth can be brokered for pulls/pushes with repository scope. |
| S3-compatible object storage | Medium / High | Proxy can sign requests or mediate bucket/key operations when clients support endpoint override. |
| Database connections | Medium | Works when applications can use a proxy endpoint and protocol support is sufficient. |
| Web applications with API backends | High | Common SaaS/API integrations can avoid raw token handoff to the workload. |

Poor proxy candidates:

| Tool / system | Why proxy is insufficient | Required fallback |
| --- | --- | --- |
| Provider CLIs that only read local config files | CLI expects a local token/config path. | Runtime lease file on tmpfs or scoped credential dir. |
| Tools that only read environment variables | No file/proxy option exists. | Final-child environment injection with redaction and no durable env files. |
| Browser/device-code human login flows | Secret is bound to interactive session state. | Explicit manual auth policy and isolated provider home. |
| SSH private key workflows | Protocol proxying is possible but not universal. | File lease with strict permissions and known_hosts policy. |

## Delivery rules

Credential proxy backend:

- The guest receives only a proxy endpoint, lease id, and policy-scoped
  reference, not the upstream provider token.
- Proxy logs must record credential id, lease id, destination, method, and
  decision, but never upstream secret values.
- Proxy policies must support allowed hosts/routes, methods, scopes, expiry,
  and optional per-session rate limits.
- Proxy bypass must be testable with egress allowlists or explicit network
  policy; otherwise a workload could call the upstream service directly with a
  leaked token from another path.

Lease file backend:

- Materialize only into runtime-scoped directories such as
  `/run/agentic-sandbox/credentials`.
- Use `0700` directory mode and narrow file modes appropriate to the provider.
- Do not write values to cloud-init, durable session records, profile records,
  command-line args, PTY metadata, or inventory/status responses.
- Prefer one lease per session/provider/use and record expiry/revocation.

Final-child environment backend:

- Use only when the provider cannot read a file or proxy endpoint.
- Set the value only for the final process, not the long-lived agent process.
- Scrub diagnostics, process listings where practical, logs, and replay
  metadata.

## Required follow-up issues

The credential proxy is needed as a major delivery backend, especially for web
applications and systems where HTTP/API/Git/S3/registry/database mediation can
avoid raw secret handoff. It is not a universal replacement for lease
materialization.

Follow-up issues filed from this decision should cover:

1. Proxy scope and threat model for ADR-028.
2. HTTP/API proxy backend implementation.
3. Git/S3/registry/database proxy adapters.
4. Proxy bypass and leakage test harness.

## Launch acceptance for #506

Issue #506 can be closed as **qualified** when:

- This document is committed as the credential posture record.
- `docs/security/attack-surface.md` references proxy and lease delivery
  boundaries.
- Proxy backend issues are filed and linked.
- Public claim language avoids absolute non-exposure claims.
