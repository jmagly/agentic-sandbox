# Changelog

All notable changes to **agentic-sandbox** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Calendar Versioning (CalVer)](https://calver.org/) in
the form `YYYY.M.PATCH` (e.g. `2026.5.0`).

## [Unreleased]

_Nothing yet._

## [2.0.0] — `<release-date>`

> **Versioning note.** Releases of agentic-sandbox use CalVer
> (`YYYY.M.PATCH`). `2.0.0` here names the **executor contract version**
> — the A2A-aligned API surface — not a CalVer tag. The CalVer release
> that first ships v2 GA will live under its own `## [YYYY.M.PATCH]`
> heading once cut. v2 is permitted as a contract identifier by ADR-018
> and the vision §7 migration discipline.

### Summary

First release of the A2A-aligned executor surface. The contract is split
across three surfaces (admin, A2A per-instance, observability — ADR-022),
routes per-instance, and ships five A2A extensions. v1 routes remain
fully functional and continue to serve existing clients; every v1
response now carries Sunset, Deprecated, and Link successor-version
headers so clients can discover the v2 path without out-of-band knowledge.

### Breaking changes

None. v1 routes still respond as they did in `2026.5.0`. The only
observable change for v1 clients is the addition of three response
headers (`Sunset`, `Deprecated`, `Link`). v1 removal is targeted for
v3.0, no earlier than 12 months after v2.0 GA (ADR-018).

### Deprecations

All `/api/v1/...` paths and the legacy v1 PTY WebSocket on port 8121
are deprecated. Removal target: **v3.0**. The default sunset date is
`Sun, 09 May 2027 00:00:00 GMT` — cited from
`management/src/http/compat_v1.rs::DEFAULT_SUNSET` and overridable per
deployment via the `AIWG_V1_SUNSET_DATE` env var (RFC 7231 IMF-fixdate;
invalid values log a warning and fall back to the default).

The full v1→v2 path map lives in code at
`management/src/http/compat_v1.rs::path_map()` and is mirrored in
`docs/v2-migration-guide.md`.

### Added

- **Three-surface architecture** (ADR-022): admin (`/api/v2/admin/*`),
  A2A per-instance (`/agents/{instance_id}/*`), observability
  (`/metrics`, `/healthz`, `/readyz`). Surfaces are non-overlapping by
  design; admin endpoints never appear under `/agents/{id}/` and vice
  versa.
- **Executor crate** (new): A2A core types, AgentCard signer (JWS over
  JCS-canonical JSON, Ed25519), per-instance router. Source of truth for
  the v2 surface; wire-compatible with [`a2a-rs`](https://github.com/a2aproject/A2A) (ADR-021).
- **A2A REST binding** — full message/task lifecycle:
  - `POST /agents/{id}/v1/messages:send`
  - `GET  /agents/{id}/v1/tasks/{tid}`
  - `GET  /agents/{id}/v1/tasks` (cursor pagination, `state=` filter)
  - `POST /agents/{id}/v1/tasks/{tid}/cancel`
  - `GET  /agents/{id}/v1/tasks/{tid}/subscribe` (SSE; replaces v1 WS mission stream)
  - `GET  /agents/{id}/v1/extendedAgentCard`
  - `POST|GET|LIST|DELETE /agents/{id}/v1/tasks/{tid}/pushNotificationConfigs[/{cid}]`
- **`pty-ws/v1` binding** — A2A-compatible PTY transport at
  `wss://host/agents/{id}/sessions/{sid}/attach`. Spec + frame schema:
  `docs/contracts/bindings/pty-ws/v1/`.
- **AgentCard discovery** at `/agents/{id}/.well-known/agent-card.json`
  — JCS-canonicalized JSON, JWS signature, declares `supportedInterfaces`
  (REST + pty-ws), `securitySchemes`, and `capabilities` including the
  five v2.0 extensions.
- **Five A2A extensions** (ADR-019 governance):
  - `runtime/v1` — declared `required: true` (enforcement deferred to v2.1)
  - `idempotency/v1` — declared `required: true`, activate to enable cache
  - `hitl-prompt/v1` — optional
  - `multi-tenant/v1` — beta; shape declared in v2.0, enforcement deferred to v2.2 (ADR-013)
  - `pty-extensions/v1` — optional
  Specs in `docs/contracts/extensions/*/v1/`.
- **Admin API** under `/api/v2/admin/*` (OpenAPI:
  `docs/contracts/admin-api.openapi.yaml`). Bearer auth (compatible with
  v1 admin tokens); mTLS + Unix-peer-creds declared in the spec for
  enforcement in v2.x (ADR-015).
- **v1 compatibility shim** (#216, #222): every v1 response carries
  `Sunset`, `Deprecated: true`, and
  `Link: <…/v2-migration-guide>; rel="successor-version"` headers.
  Prometheus counter `aiwg_v1_path_requests_total{path}` per v1 hit so
  operators can prioritise migration work. Sunset date configurable via
  `AIWG_V1_SUNSET_DATE`.
- **Conformance harness** (#217 — separate repo:
  [`roctinam/agentic-sandbox-conformance`](https://git.integrolabs.net/roctinam/agentic-sandbox-conformance)).
  Runs against any executor URL, asserts contract conformance, emits
  markdown + JUnit reports.
- **Migration guide** at [`docs/v2-migration-guide.md`](docs/v2-migration-guide.md).
  Canonical reference for the v1→v2 path map, AgentCard discovery,
  extension activation, auth changes, and sunset timeline.

### Sunset

- Default `Sunset` date for all `/api/v1/...` routes:
  `Sun, 09 May 2027 00:00:00 GMT` (see
  `management/src/http/compat_v1.rs::DEFAULT_SUNSET`).
- Override per deployment: set `AIWG_V1_SUNSET_DATE` to an RFC 7231
  IMF-fixdate string.
- v3.0 removes v1 routes entirely. No earlier than 12 months after v2.0 GA.

### Migration

See [`docs/v2-migration-guide.md`](docs/v2-migration-guide.md).

### References

- [ADR-018 — A2A as base protocol](.aiwg/architecture/adr/ADR-018-a2a-as-base-protocol.md)
- [ADR-019 — Extension URI scheme and governance](.aiwg/architecture/adr/ADR-019-extension-uri-scheme-and-governance.md)
- [ADR-020 — PTY custom protocol binding](.aiwg/architecture/adr/ADR-020-pty-custom-protocol-binding.md)
- [ADR-021 — `a2a-rs` as wire dependency](.aiwg/architecture/adr/ADR-021-a2a-rs-as-wire-dependency.md)
- [ADR-022 — Three-surface architecture](.aiwg/architecture/adr/ADR-022-three-surface-architecture.md)

## [2026.5.0] — 2026-05-08

First tagged release. Captures the work that took the management server,
dashboard, and AIWG bridge to the first known-good baseline operators
can reference for further work.

### Highlights

| What changed | Why you care |
|---|---|
| **Container runtime parity with VM agentic-dev** (#181 epic, #182–#186) | Spawn an agent container from the dashboard and immediately use Python / Node / Go / cargo / rg without `apt install`. New `agentic/agent:dev` shared toolchain layer feeds rebased `claude` / `codex` / `opencode` images. Smoke matrix runs in CI. |
| **Unified Instances surface in the dashboard** (#178) | One Create dialog with a Runtime dropdown (VM \| Container). Combined sidebar list with `[VM]` / `[CT]` runtime badges. Per-row controls match each runtime's real lifecycle — no phantom buttons. |
| **AIWG bridge handshake works end-to-end** (#190, #191, #192) | Server emits a `server_hello` capability banner so AIWG's `replayCapable` gate flips; `create_session` REST response self-describes the actual WS flow; `agent_sessions` event pushes per-agent session inventory so AIWG can render counts without per-instance polling. |
| **PTY rendering corruption recovery** (#180 phases 1–4) | Floor + debounce + dual-frame stability check on `pty_resize` (UI), server-side reject below 20×5 (defense-in-depth), `term.reset()` on every session attach to defeat reconnect-state drift, and a manual `⟳ Resync` button as the operator-side escape hatch. |
| **Observability for the next recurrence** (#188 sections A–C) | `libvirt_blocking` logs every RPC's duration (warn >1 s, error >5 s); `JoinSession` logs attempt + replay window + result; `pty_resize` accept/drop traces in both UI console and `mgmt.log`. |
| **Provisioning host.internal survives reboot** (4707e4e + b80dc06) | systemd oneshot replaces the cloud-init runcmd that only fired on first boot. Agent VMs now reconnect to the management server cleanly across host reboots. |
| **Container UX safety** (a5c897f, 005e471, 24e1cf9, 2e76a0d, 9dd7711) | Stop button no longer destroys; Force-off ≠ Delete; orphan-cleanup default flipped off and prefix tightened to `task-` so operator-provisioned `agent-*` VMs can't be wiped. Container create auto-injects the agent bootstrap env. |
| **Raw logs panel + filterable Events** (24e1cf9) | New `GET /api/v1/logs` reads from an in-memory tracing ring buffer; SSE on `/api/v1/events?follow=true` for live event streaming; both panels filterable by level + type/target with auto-populated dropdowns. |

### Added

- **`agentic/agent:dev` shared dev base** (#182): Python (uv), Node (fnm), Go, Rust (rustup minimal), ripgrep, fd, bat, eza, jq, delta, xh, grpcurl, cmake, ninja, meson, gcc, make, aider (pinned to Python 3.12 — pydub→audioop on 3.13), gh + built-in `gh copilot`. /etc/profile.d snippet keeps PATH stable across login shells.
- **Container variants rebased on `agent:dev`** (#183, #184, #185): claude / codex / opencode FROM `agent:dev`. Image-size note: ~3.3–4.0 GB per platform, larger than the original 1.5 GB estimate but acceptable for v1.
- **CI build + publish + smoke matrix for agent images** (#186): `.gitea/workflows/ci.yaml` builds `base → dev → claude/codex/opencode` with registry buildcache, pushes on main, and runs `tests/container/smoke.sh` against each variant.
- **Container runtime UI** (#178): unified Create Instance dialog, combined Instances sidebar with runtime badges, per-runtime pane controls (Stop / Delete for containers; Restart / Stop / Force-off / Delete for VMs).
- **`GET /api/v1/container-images` endpoint** (#179): curated list of agent container images for the dashboard image picker.
- **`GET /api/v1/logs` + in-memory tracing ring buffer** (#188 follow-on): dashboard System tab consumes this for raw server logs.
- **WS `server_hello` capability banner** (#190): first frame on every connection lists `supported_client_messages` and `features` so clients (AIWG bridge, future tooling) can feature-gate without probing.
- **`SandboxEvent::AgentSessions`** (#192): authoritative session inventory pushed to AIWG after `AgentConnected` (initial), and after every `SessionStart` / `SessionEnd` (atomic re-broadcast).
- **`⟳ Resync` button per pane** (#180 phase 4): manual escape hatch — `term.reset()` + refit + drop stored seq + re-attach.
- **Live event SSE via `/api/v1/events?follow=true`** (24e1cf9): dashboard Events tab streams + falls back to 5s polling.
- **HITL ANSI strip** (ce5136b): popup context no longer carries raw VT escape codes.
- **provisioning(loadout) flow** with full-suite, claude-only, dual-review, security-audit, etc. variants (`images/qemu/loadouts/profiles/`).

### Changed

- **`Stop` button** in the dashboard now does graceful shutdown only (`POST /vms/{name}/stop`); previously it destroyed and deleted the disk. New `⏻ Force off` (`POST /vms/{name}/destroy`) does hard power-off without delete; `✕ Delete` is its own action with a confirmation that warns about disk wipe (a5c897f, 24e1cf9).
- **Orphan-VM cleanup defaults** (#187 prereq, 2e76a0d): `RetentionPolicy::cleanup_orphaned_vms` flipped to `false` (opt-in); `managed_vm_prefix` is configurable and defaults to `task-`. Operator-provisioned `agent-*` VMs are no longer eligible for orphan cleanup.
- **`POST /api/v1/agents/:id/sessions`** response shape (#191): `ws_url` (which pointed at a route that didn't exist) replaced with `ws_endpoint` + `join_message` so the contract self-describes the actual flow.
- **`pty_resize` floor** raised to `cols ≥ 60, rows ≥ 10` on the UI side, with 150 ms debounce and a two-`requestAnimationFrame` stability check (#180 phases 1+2). Server-side reject at `< 20 × 5` (defense-in-depth).
- **Container spawn flow** auto-injects `MANAGEMENT_SERVER`, `AGENT_ID`, `AGENT_SECRET` env (9dd7711) and `--add-host host.docker.internal:host-gateway`; mints the secret via `SecretStore` so the agent's first connect goes through verify-primary, not the auto-register fallback. Previously containers exited 1 immediately because the entrypoint required these env vars.
- **`attachToSession`** in the dashboard now always calls `term.reset()` before the join_session message (#180 phase 3). Brief flash beats corrupted rendering — was the cause of stacked status bars + overlapping output on multi-window tmux reconnects.
- **`libvirt_blocking`** measures every RPC and logs duration (#188 section A): warn >1 s, error >5 s.
- **WS `JoinSession` handler** logs attempt, success with `replay_window`, and rejects (#188 section B); UI mirrors with `console.log` at `attachToSession`.
- **`pty_resize` accept/drop logging** at INFO with `reason=` (#188 section C); was DEBUG and invisible by default.

### Fixed

- **PTY display corruption after extended sessions** (#180): stacked tmux status bars + overlapping output on multi-window tmux + reconnect chains. Root cause was xterm state-machine drift across WS reconnects against a delta-replay against stale state.
- **Stop button destroying VMs** (a5c897f): was calling DELETE with `force=true&delete_disk=true`; now hits `/stop`.
- **`/api/v1/vms` hanging when libvirt is sluggish** (#187 — partial; per-call timeout still pending): documented and tracked. Recovery via `systemctl restart libvirtd` (qemu processes survive).
- **`host.internal` lost across VM reboots** (4707e4e + b80dc06): cloud-init `manage_etc_hosts: True` was regenerating `/etc/hosts` on each boot, dropping the runcmd-added entry. New `agentic-hosts.service` systemd oneshot reasserts the entry on every boot. Also fixed the heredoc-escape and ordering-cycle that snuck through the first attempt.
- **Container session crashing on first start** (9dd7711): missing `MANAGEMENT_SERVER` / `AGENT_ID` / `AGENT_SECRET` env. Backend now injects defaults if not provided.
- **HITL popup carrying raw escape codes** (ce5136b): `strip_ansi` helper covers CSI, OSC, DCS, two-byte ESC sequences, BEL/NUL.
- **Orphan-cleanup helpers wiping operator VMs** (2e76a0d): hardcoded `agent-` prefix in `cleanup_orphaned_vms` would wipe all operator VMs once enabled; replaced with configurable prefix defaulting to `task-`, and refuses to enumerate when the prefix is empty.
- **`pty_resize` falling back to 80×24 on degenerate fit()** (a5c897f, 005e471): was the original cause of the "1/4 screen" rendering bug.

### Deferred

- **`/api/v1/vms` per-call timeout + circuit breaker** (#187 phase 1): `libvirt_blocking` still has no upstream timeout; only the Axum-level cutoff. Workaround documented (`systemctl restart libvirtd`); fix lands in next series.
- **Dashboard "libvirt degraded" fallback** (#189): when `/api/v1/vms` is unresponsive, surface gRPC-connected agents from `/api/v1/agents` with a degraded chip rather than rendering "0 VMs."
- **Observability sections D / E / F** (#188): registry-divergence detector, `/healthz/libvirt` health surface, per-line `client_id` tags. Sections A / B / C shipped in `2192840`.
- **AIWG-side consumers** (aiwg#1144, aiwg#1146, aiwg#1148, aiwg#1151) — independent of this baseline.

### Operator notes

- Container images need to be rebuilt (`images/container/build.sh`) or pulled from CI registry to pick up the parity work.
- VM `host.internal` persistence requires a re-provision (existing VMs with the old cloud-init won't have the systemd oneshot until re-provisioned).
- AIWG bridge: requires a sandbox running this version or later for `replayCapable` to flip true.

[Unreleased]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.0...HEAD
[2.0.0]: ./docs/v2-migration-guide.md
[2026.5.0]: https://git.integrolabs.net/roctinam/agentic-sandbox/releases/tag/v2026.5.0
