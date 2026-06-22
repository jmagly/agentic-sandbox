# Attack surface inventory

Date: 2026-06-19

Scope: launch review inventory for agentic-sandbox management, agent, VM,
container, credential, filesystem, network, and release/build surfaces. This
document closes the documentation deliverable for Gitea issue #505.

## Status legend

| Status | Meaning |
| --- | --- |
| Default | Present in the default local-first deployment path. |
| Optional | Present only when a runtime, profile, or feature is enabled. |
| Deprecated | Historical or compatibility path that should not be used for launch. |
| Planned | Architecture direction exists, but implementation is not complete. |
| Open | Known gap or verification item. |

## Management service surfaces

| Surface | Default binding | Purpose | Authentication / identity | Status | Notes |
| --- | --- | --- | --- | --- | --- |
| gRPC `8120` | `127.0.0.1:8120` by code default | Agent control stream and bootstrap-related control plane | UDS peer credentials, vsock identity, or mTLS client identity for secure agent paths | Default | Plain TCP has no transport identity and is rejected for agent identity. Non-loopback listener profiles require explicit verification. |
| WebSocket `8121` | Derived from gRPC bind | Realtime metrics and telemetry | Local-host operator access; no general remote admin auth claim | Default | Treat as local operator-only unless placed behind an authenticated reverse proxy. |
| HTTP REST and dashboard `8122` | Derived from gRPC bind | Dashboard, REST API, health, admin, containers, credentials, startup profiles | Local-host operator access; selected session dispatch requires bearer token | Default | Do not expose directly on untrusted networks. |
| Metrics over HTTP `8122` | Same HTTP listener | Prometheus-style scrape and health visibility | Same listener boundary as HTTP API | Default | Metrics can disclose inventory and state; keep local or gated. |
| Deployment compose host ports | `127.0.0.1:8120-8122` in production compose | Containerized management access | Host loopback publish; container may listen on `0.0.0.0` internally | Optional | Host publish boundary is the security boundary for production compose. |

## Agent transport surfaces

| Transport | Purpose | Identity model | Status | Launch boundary |
| --- | --- | --- | --- | --- |
| Unix domain socket | Local/container agent connection | Peer credentials | Default / Optional | Preferred for same-host local agents. |
| vsock | Host-to-VM control channel | CID/port identity context | Optional | Preferred VM local channel where available. |
| mTLS | Remote or bridged agent control | Client certificate and server CA | Optional | Required when crossing a network boundary. |
| TCP without transport identity | Legacy/plain connectivity | None | Deprecated | Not acceptable for launch claims of authenticated agent control. |
| Legacy `x-agent-secret` / `AGENT_SECRET` | Historical bearer agent auth | Shared secret | Deprecated | Retired; docs and diagrams should not present it as current architecture. |

## HTTP API surfaces

| API family | Purpose | Sensitive operations | Status | Notes |
| --- | --- | --- | --- | --- |
| `/api/v1/sessions` | Session lifecycle and dispatch | Command dispatch, session control | Default | Dispatch requires bearer token from `aiwg serve`; other local operator operations rely on local binding. |
| `/api/v2/admin` | Admin and runtime control | Runtime state, bootstrapping, legacy retirement notices | Default | Legacy shared-secret rotation returns retired/gone. |
| `/api/v2/credentials` | Credential metadata and lease management | Write-only credential metadata, lease issue/revoke | Default / Emerging | API must never return secret values. Treat lease materialization as sensitive workload handoff. |
| `/api/v2/startup-profiles` | Startup policy | Credential refs, command/profile wiring | Default / Emerging | Stores metadata refs, not provider token blobs. |
| `/api/v1/containers` and related image APIs | Container runtime launch | Environment and transport bootstrap material | Optional | Requires profile-specific validation of env redaction and bootstrap token lifetime. |
| Static dashboard UI | Operator browser UI | Session state and controls | Default | UI sink audit and CSP hardening remain launch follow-ups. |

## Runtime and isolation surfaces

| Runtime | Boundary | Host touchpoints | Status | Notes |
| --- | --- | --- | --- | --- |
| QEMU/KVM VM | Hardware virtualization plus configured shared storage/network | libvirt/qemu, cloud-init ISO, virtiofs/9p-style shares, vsock/mTLS | Default / Optional | Strongest isolation path. Base image, seed ISO, and loadout hashes are recorded in VM metadata. Mount flags and seclabel/sVirt remain verification items. |
| Container runtime | Namespace/cgroup boundary | Docker/rootless Docker, image entrypoint, env/mounts | Optional | Dev test compose drops caps and uses read-only FS. Other images and Dockerfiles need release verification. |
| Host-direct agent | Process boundary on host | Host filesystem, UDS, env | Optional | Treat as trusted local automation rather than tenant isolation. |
| Future remote/cloud runtimes | Network boundary | mTLS, provisioning APIs, workload credentials | Planned | Requires remote auth, policy, and evidence beyond local-first launch posture. |

## Network surfaces

| Surface | Direction | Status | Risk / control |
| --- | --- | --- | --- |
| Host loopback management plane | Operator to management | Default | Primary launch assumption. Keep local or behind authenticated reverse proxy. |
| VM bridge or vsock | VM agent to management | Optional | Use vsock or mTLS; avoid plaintext bridged TCP. |
| Guest workload egress | Agent workload to internet or internal services | Optional | Needs explicit network profile: isolated, allowlist, or full egress. |
| Docker network | Containers to management and external services | Optional | Host port publishing and network policy define exposure. |
| Docker API socket | Management to Docker daemon | Optional | Raw `docker.sock` is not mounted by default dev compose; use a restricted socket proxy if needed. |
| Credential proxy | Workload to upstream web/API/Git/S3/registry/database service through host broker | Planned | Delivery backend under ADR-028. Useful when protocol mediation avoids placing raw secrets in the guest; requires lease-bound policy, audit redaction, and egress/bypass controls before non-exposure claims. Not suitable for every provider or CLI. |

## Filesystem and mount surfaces

| Surface | Contents | Status | Required controls |
| --- | --- | --- | --- |
| Shared workspace / agentshare | Project files, agent output, inbox/drop areas | Default | Principle of least write access; clear read-only vs writable paths; avoid sharing credential directories. |
| Cloud-init seed ISO | Bootstrap config, mTLS paths or one-time enrollment token | Optional | Treat as sensitive until bootstrapped; restrict host permissions; detach or clean where practical. |
| `/etc/agentic-sandbox/grpc-mtls` in guest | Agent mTLS CA/cert/key | Optional | Cert/key files must use restricted ownership and private key mode. |
| `/run/agentic-sandbox/credentials` in guest | Session-scoped credential lease files | Emerging | Runtime dir mode `0700`; values must not be durable or logged. |
| Credential refs policy | Credential ids, provider, allowed use, target hints | Emerging | Metadata only; reject inline `value` or secret-like fields. |
| Base qcow2 / overlays | VM base image and per-session overlay | Default / Optional | Pin ISO/base hashes, record manifest, verify backing chain. |
| Docker volumes and tmpfs | Container state and temp storage | Optional | Prefer tmpfs for transient secrets and read-only root filesystems where feasible. |

## Credential surfaces

| Surface | Secret exposure model | Status | Decision |
| --- | --- | --- | --- |
| Credential metadata API | Stores ids, provider, allowed use, and backend refs | Emerging | Secret values are write-only and must not be returned in API responses. |
| Startup profiles | Persist credential references | Emerging | Profiles must not store env blobs or token values. |
| Agent credential ref contract | Agent receives metadata refs and target hints | Emerging | Inline secret values are rejected by parser tests. |
| Lease file materialization | Workload receives local file secret | Emerging | Required for provider CLIs and tools that need local files. Use tmpfs/runtime-scoped paths. |
| Final-child environment materialization | Workload receives env var only at final process launch | Emerging | Last resort for tools with no file or proxy option. Never place values in durable env files, command args, logs, or inventory. |
| Credential proxy backend | Workload calls upstream through broker | Planned | Preferred for web/API/Git/S3/registry flows where clients can target a proxy. Database support is feasibility-gated. Provider CLIs, env-only tools, SSH private keys, and browser/device-code login state may still require file or final-child-env delivery. |
| Manual interactive auth | Human signs in through provider UI/CLI | Optional | Allowed only as explicit policy exception with no false non-exposure claim. |

## Logging and audit surfaces

| Surface | Potentially sensitive data | Status | Controls |
| --- | --- | --- | --- |
| Management logs | Session ids, transport errors, bootstrap events | Default | Redact bootstrap tokens, credentials, and command args. |
| Agent logs | Provider readiness, command status, transport diagnostics | Default | Provider readiness may report presence, never values. |
| PTY transcript and replay metadata | User and agent terminal output | Optional | Treat as sensitive by default; do not include credential values or lease material in metadata. |
| Inventory/status APIs | Runtime state, image refs, profiles, credential metadata | Default | Never include provider token values, private keys, or bootstrap tokens. |
| Crash and cleanup paths | Residual lease files, overlays, seed ISOs | Open | Need explicit crash-path revocation and cleanup verification. |
| Credential proxy audit | Lease use, upstream target, action, decision, status class | Planned | May log credential id, lease id, session id, adapter, host, route/action, and redaction profile. Must not log upstream bearer values, API keys, cookies, signed URLs, private keys, or full bodies without a redacted capture policy. |

## Release and build surfaces

| Surface | Status | Required launch evidence |
| --- | --- | --- |
| CI workflows | Partial | SHA-pinned actions and no deprecated actions in release-critical workflows. |
| Container base images | Partial | Digest-pinned `FROM` lines or explicit exclusion from release. |
| npm/global installers | Partial | Version-pinned active release paths; backup and legacy paths excluded or linted. |
| ISO and qcow2 images | Partial | Populated ISO pins, verified downloads, qcow2 manifest, backing-chain verification, and VM metadata hashes. |
| Loadout manifests | Partial | Source and resolved manifest hashes recorded in VM metadata. |
| Rust/Python dependencies | Open | Dependency vulnerability and lockfile verification evidence. |

## Launch-safe external wording

Use:

- "Local-first management plane with loopback defaults and secure agent
  transport options."
- "Agent control can use UDS, vsock, or mTLS transport identity."
- "Credential APIs and startup profiles use metadata references instead of
  returning or persisting provider token values."
- "Credential delivery is policy-driven: proxy where protocols allow it,
  materialize short-lived files or final-process env only when tools require
  local secrets."
- "Proxy-backed credential delivery is a planned ADR-028 backend for protocols
  that can be mediated; it requires lease policy, audit redaction, and bypass
  controls before supporting non-exposure claims."

Avoid until follow-ups close:

- "All management APIs are remotely authenticated."
- "No secrets ever enter a VM or container."
- "All images and build inputs are fully pinned and reproducible."
- "The browser UI is CSP-hardened and XSS-audited."
- "Crash cleanup guarantees credential revocation in every runtime."
