# Changelog

All notable changes to **agentic-sandbox** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Calendar Versioning (CalVer)](https://calver.org/) in
the form `YYYY.M.PATCH` (e.g. `2026.5.0`).

## [Unreleased]

## [2026.7.12] — 2026-07-13

An agent-reliability fix (#637) plus the build-performance retune. First release
compiled on the faster thin-LTO / parallel-codegen profile; the full pipeline
runs on the pure-vault + variables setup proven in 2026.7.11.

### Fixed

- **Agent output survives reconnect recovery (#637).** The agent's
  `recover_output_channel` recreated the output channel on a 5s timeout, swapping
  the sender that every running session captured at spawn — after which those
  sessions silently dropped **all** future output while still appearing healthy
  and re-adopted. It now waits for the forwarder's guaranteed receiver hand-back
  instead of swapping (the transport is already torn down, so the hand-back is
  prompt). The timeout is now a 30s deadlock guard that recreates the channel
  loudly — and states the true blast radius (degraded sessions) — only if the
  forwarder ever wedges.

### Added

- **First regression tests for the #633/#634 control-channel range** (run in CI):
  output-channel recovery preserves session senders, the last-resort fallback
  installs a working channel, `SessionReport` dedup (tmux alias identity
  migration, via an extracted `merge_discovered_sessions`), and the keepalive
  constants mirror the server listeners.

### Changed

- **Faster CI builds.** The release profile moved from fat `lto=true` /
  `codegen-units=1` (single-threaded codegen that left the runner cores idle and
  OOM-killed concurrent jobs) to `lto="thin"` / `codegen-units=16`, with
  `CARGO_BUILD_JOBS=12`. Negligible runtime cost for a service/agent.
- **`CF_ZONE_ID` is now a Gitea repository variable**, not an Actions secret — it
  is a non-secret Cloudflare zone id. Only secret-zero (`BAO_CI_ROLE_ID` /
  `BAO_CI_SECRET_ID`) remains a secret; all other config is in variables.

### Documentation

- `docs/DEPLOYMENT.md` documents agent-version propagation: the agent fix only
  reaches VMs whose binary is redeployed (`deploy-agent.sh`) or reprovisioned — a
  pre-v2026.7.7-image VM still runs the old kill-on-reconnect agent, and
  vsock/static-IP apply only to newly provisioned VMs.

### Operator notes

- No management-server behaviour change. Redeploy or reprovision VMs to pick up
  the #637 agent fix.
- The titan CI runner is now capped at one build / ≤12 cores at a time.

## [2026.7.11] — 2026-07-12

CI/docs release — runtime binaries are **identical** to 2026.7.10. This bump
re-runs the full pipeline to verify it end to end after moving non-secret config
out of Actions secrets, and adds automatic Cloudflare cache purging on docs
publish.

### Added

- **Cloudflare cache purge on docs publish.** After the docsite rsync,
  `docsite-deploy.yml` fetches the Cloudflare API token from OpenBao
  (`kv_internal/ci/shared/cloudflare-api`) and purges the cache for the
  `docs.aiwg.io/agentic-sandbox/` subpath — purge-by-URL, batched 30/request,
  scoped to the tenant (never the root or sibling tenants). Gated on
  `CF_ZONE_ID`; skips gracefully when unset. Verified live: 517 URLs / 18
  batches, all successful.

### Changed

- **Non-secret docsite deploy config moved to Gitea repository variables.**
  `DEPLOY_HOST` / `DEPLOY_PATH` / `DEPLOY_PORT` / `DEPLOY_USER` are host
  coordinates, not secrets — `docsite-deploy.yml` now reads them from `vars.*`.
  Only secret-zero (`BAO_CI_ROLE_ID` / `BAO_CI_SECRET_ID`), the Cloudflare zone
  id, and vault-sourced key material remain as Actions secrets.

### Documentation

- Release runbook's secret/variable table updated to reflect the vault +
  variables split: the docs deploy key and Cloudflare token are vault-sourced;
  `DEPLOY_*` are repository variables.

### Operator notes

- No binary or behavioral change vs 2026.7.10. This release verifies the
  complete pipeline runs purely on OpenBao (secrets) + Gitea variables (config):
  internal registry publish, GHCR + GitHub mirror, docsite deploy + cache purge,
  and GPG signing with `9292EFCB…E09C33`.
- The now-redundant `DEPLOY_HOST/PATH/PORT/USER` and `DEPLOY_SSH_KEY` Actions
  secrets can be deleted — nothing references them.

## [2026.7.10] — 2026-07-12

Completes the signed release. Runtime is **identical** to 2026.7.7–2026.7.9
(#633/#634); this bump re-runs the pipeline after rotating the release-signing
key so tarballs are GPG-signed, images cosign-signed, and the SBOM is generated.

### Security

- **Release-signing key rotated** (#636). The previous shared key
  (`FE9272F0…E84CE8`) was protected by a personal user passphrase that could not
  be used for headless CI signing and did not belong in a shared vault. Replaced
  with a **dedicated CI ed25519 key** whose machine-generated passphrase is
  stored vault-only. New identity for verifiers:
  - fingerprint `9292EFCBB0EA41BECEEFDAFA9C1B8CE0E0E09C33`, keyid `9C1B8CE0E0E09C33`
  - public key: `docs/releases/keys/agentic-sandbox-release-key.asc`
  The previous key produced no published signatures, so nothing needs
  re-verification. This is a **cross-project** signing key (AIWG and others share
  the same vault path and will sign with the new key).

### Fixed

- Headless GPG signing now completes: `sign-and-sbom` forces loopback pinentry
  and feeds the vault-stored passphrase, producing `.asc` signatures + SBOM.

### Operator notes

- All CI secrets are sourced from OpenBao at job time (#635) via the
  `ci-agentic-sandbox` AppRole; retired Gitea value secrets can be removed.
- Verifiers must import the **new** public key (fingerprint above); signatures
  from v2026.7.10 onward use it.

## [2026.7.9] — 2026-07-12

> **Superseded by [2026.7.10].** Published binaries + public GHCR images but no
> GPG signatures (signing key required a personal passphrase). 2026.7.10 rotates
> the key and ships the complete signed artifacts; identical runtime.

Completes the release pipeline that [2026.7.8] left short. Runtime is **identical**
to 2026.7.7/2026.7.8; this bump re-runs the pipeline with a GPG-signing fix so the
release carries `.asc` signatures, cosign image signatures, and the SBOM.

2026.7.8 published binaries and public GHCR images but its `sign-and-sbom` job
failed: after importing the (passphrase-less) release key from OpenBao, gpg fell
back to an interactive pinentry in the headless runner and errored with "Screen
or window too small".

### Fixed

- **Headless GPG signing** — the `sign-and-sbom` job now writes
  `allow-loopback-pinentry` / `pinentry-mode loopback` / `batch` / `no-tty` into
  its ephemeral GNUPGHOME so gpg never invokes an interactive pinentry. The
  passphrase-less release key signs directly.

### Operator notes

- The OpenBao secret migration (#635) is fully in effect and verified: all CI
  secrets (registry, GHCR, GitHub-mirror, docsite, mutsu, and the GPG signing
  key) are fetched from the vault at job time via the `ci-agentic-sandbox`
  AppRole. Signing identity unchanged (`FE9272F0BC5781E1DE77FAAA719AB63879E84CE8`).
- The retired Gitea value secrets can be removed now that every lane is green.

## [2026.7.8] — 2026-07-12

> **Superseded by [2026.7.9].** This tag published binaries + public GHCR images
> but no GPG signatures / SBOM (headless-pinentry bug, fixed in 2026.7.9). Use
> 2026.7.9 — identical runtime, complete signed artifacts.

Release-publication fix that supersedes [2026.7.7]. The runtime is **identical**
to 2026.7.7 (the #633/#634 VM control-channel fixes); this bump exists only to
re-run the release pipeline with a CI fix so the published release is complete.

2026.7.7's tag pipeline published binaries but was missing GPG signatures, the
SBOM, and public GHCR container images: after #635 moved CI secret fetching to
OpenBao, the `multi-registry-push` job (which re-tags/pushes prebuilt images and
did not check out the repo) failed to find `ci/openbao-fetch.sh`, and
`sign-and-sbom` is gated behind it, so both were skipped. 2026.7.8 carries the
checkout fix and completes the OpenBao-backed release, mirror, and signing lanes.

### Fixed

- **`multi-registry-push` now checks out the repo** (#635): the GHCR/Quay mirror
  job fetches its token via `ci/openbao-fetch.sh`, which requires the repo on
  disk. Adds the missing `actions/checkout`, unblocking the public GHCR mirror
  (#299/#478) and, transitively, `sign-and-sbom` (#300).

### Operator notes

- All CI secrets now source from OpenBao at job time via the `ci-agentic-sandbox`
  AppRole "secret zero" (`BAO_CI_ROLE_ID`/`BAO_CI_SECRET_ID`): internal-registry
  creds, GHCR token, GitHub mirror token, docsite deploy key, mutsu SSH key, and
  the GPG release-signing key (fingerprint unchanged,
  `FE9272F0BC5781E1DE77FAAA719AB63879E84CE8`). The retired Gitea value secrets
  can be removed now that every lane is verified green.
- Verifiers: signatures are produced by the same key as before; verify as usual.

## [2026.7.7] — 2026-07-12

> **Superseded by [2026.7.8].** This tag's release pipeline published binaries
> but not GPG signatures, the SBOM, or public GHCR images (CI checkout bug, fixed
> in 2026.7.8). Use 2026.7.8, which ships the identical runtime plus the complete
> signed/mirrored release artifacts.

Reliability fixes for the VM runtime control channel and session lifecycle,
and a supply-chain change that moves release signing to OpenBao. This is the
first release whose GPG signatures are produced from a key fetched from the
vault at CI time rather than a stored CI secret — the signing identity
(fingerprint `FE9272F0BC5781E1DE77FAAA719AB63879E84CE8`) is unchanged for
verifiers.

Reliability fixes for the VM runtime control channel and session lifecycle,
and a supply-chain change that moves release signing to OpenBao. This is the
first release whose GPG signatures are produced from a key fetched from the
vault at CI time rather than a stored CI secret — the signing identity
(fingerprint `FE9272F0BC5781E1DE77FAAA719AB63879E84CE8`) is unchanged for
verifiers.

### Added

- **vsock control transport enabled by default for same-host VMs** (#633):
  when the host exposes `/dev/vhost-vsock`, the management server now serves
  the vsock gRPC listener and provisioning selects it, so guest↔host control
  traffic no longer crosses the libvirt NAT/ufw boundary. Opt out with
  `AGENTIC_GRPC_VSOCK_PORT=0`; hosts without vsock keep the mTLS-TCP path.
- **Static IP pinned for loadout-provisioned VMs** (#633): loadout VMs now
  receive a MAC-matched static `network-config` like the profile paths,
  removing in-guest DHCP (and its ~30-min lease renewal) from provisioned
  guests.

### Fixed

- **VM control channel dropped after ~30 min idle** (#633): the agent gRPC
  client now sets HTTP/2 keepalive (10s/20s, ping-while-idle) plus OS TCP
  keepalive on every transport, so a silently torn-down flow becomes a fast
  transport error that trips the existing reconnect/backoff instead of a
  zombie connection.
- **Sessions unrecoverable after a VM agent reconnect** (#634): transport
  reconnect is now state-preserving — the agent no longer SIGTERMs tracked
  workloads on stream loss, the output channel survives reconnects, and
  server-side reconcile is the sole kill authority. Non-tmux sessions now
  traverse a reconnect instead of being killed.

### Changed

- **Release GPG signing key sourced from OpenBao** (release `sign-and-sbom`
  job): CI logs into OpenBao with a least-privilege AppRole (its role-id /
  secret-id are the only stored CI secret) and reads the release key
  ephemerally at signing time, replacing the `GPG_PRIVATE_KEY` /
  `GPG_PASSPHRASE` CI secrets. Signature identity is unchanged. cosign image
  signing is unaffected (still a CI secret).

### Documentation

- Release runbook documents the OpenBao-backed signing prerequisite and the
  CI reader AppRole; verification doc publishes the expected key fingerprint.

## [2026.7.6] — 2026-07-11

Session delivery to external consumers. This release ships the structured
agent-output chat stream for AIWG Cockpit (#600) and closes the admin-UX
audit that followed it (#632): the dashboard now surfaces the same
session-delivery capabilities external clients rely on, and the AgentCard
advertises them for discovery.

### Added

- **AgentCard advertises the structured agent-output/chat capability** (#630):
  the signed AgentCard now includes an `agent-output/v1` extension (sources,
  event vocabulary, Fortemi-compatible envelope) and a `supportedInterfaces`
  entry for the SSE chat endpoint, so capability discovery via the card — used
  by Cockpit and the admin AgentCard panel — can find the #600 stream, not just
  the per-session `chat_source`. Contract doc at
  `docs/contracts/extensions/agent-output/v1/spec.md`. Completes #600's
  AgentCard acceptance.
- **Admin dashboard: Sessions & Output panel** (#628, #629): the agent detail
  modal now surfaces each session's delivery capabilities (`chat_source`,
  `session_backend`, `session_class`, screen availability), a read-only SSE
  **Chat** viewer over `/api/v1/agent-output/chat` (normalized
  message/tool/status events, `Last-Event-ID` resume), a **Transcript** view,
  and a **Screen** inspector — closing the operator-parity gap with Cockpit.
- **Admin dashboard: VM Reprovision control** (#631): confirmation-guarded,
  operation-tracked Reprovision button on running VMs. The AIWG-reconnect
  control tooltip now disambiguates it from the #625 container control-stream
  reconnect (CLI-only). (rotate-secret remains intentionally retired, #412.)
- **Structured agent-output chat stream for Cockpit** (#600): new read-only
  `GET /api/v1/agent-output/chat` endpoint projects a command's Claude Code
  `stream-json` output into normalized message/tool-call/tool-result/status
  events, so Cockpit's Chat view no longer has to scrape PTY bytes. Raw output
  (`/api/v1/agent-output/stream`) stays authoritative; subscribing to the chat
  projection confers no controller input authority. Frames follow the Fortemi
  `POST /api/v1/chat/stream` SSE envelope (named events, JSON `data`, monotonic
  `{session}-{seq}` ids, `Last-Event-ID` resume, `STREAM_INTERRUPTED`
  terminal) for cross-project wire compatibility — `delta`/`done`/`error` are a
  Fortemi-compatible subset, `tool_call`/`tool_result`/`status`/`raw` are
  additive. Session APIs now advertise `chat_source` (`stream-json` | `none`)
  and a `chat_stream_url`; Codex structured output is tracked as follow-up.
  Convergence on a shared agent-chat schema is tracked in `Fortemi/fortemi`.

## [2026.7.5] — 2026-07-10

v2026.7.5 is a control-plane reliability hotfix. A wedged-libvirt (VM-path)
stall can no longer reap healthy Docker/host agents, reaped-but-alive agents
recover, and operators can force a live container to reconnect without losing
running work.

### Added

- **Operator-initiated reconnect for a live container** (#625): `agent-client`
  now handles `SIGHUP` to tear down its control stream and re-register —
  re-adopting existing tmux sessions (#613) — **without exiting the process or
  stopping the container**. Ships `agent-reconnect` in every agent image (via
  `Dockerfile.base`); run `docker exec <ctr> agent-reconnect` (or
  `docker kill --signal=HUP <ctr>`) to recover a soft-locked agent without
  losing running work. The helper only signals the existing client and never
  spawns a competing one.

### Fixed

- **libvirt stalls no longer reap healthy Docker/host agents** (#623, critical):
  the stale reaper now defers to control-stream liveness (`is_stream_connected`)
  as ground truth. A wedged-libvirt `admin_v2.instances.list` stall that starves
  heartbeat ingestion can no longer disconnect/unregister a live, still-connected
  agent — which previously stranded its running sessions from the control plane
  (Cockpit "soft-lock"). Complements the libvirt-call isolation + circuit breaker
  from v2026.7.4.
- **Reaped-but-alive agents recover** (#624): re-registration of a
  previously-removed agent-id is accepted and reconciles reported sessions
  (a fresh registration re-drives `SessionQuery` → #613 tmux adoption).
  Combined with the reaping guard above and the operator reconnect (#625), a
  returning agent is reinstated without a fresh instance.

### Operator notes

- CI: the heavy release-artifact builds on the shared `titan` runner are now
  serialized — `release-linux-packages` runs after `release-binaries` (so the
  two x86_64 management/package builds never overlap), and a top-level
  `concurrency` group queues overlapping runs. This prevents the
  runner-saturation "killed without logs" build failures. The definitive
  cross-workflow fix remains act_runner `capacity: 1` on the titan host.

## [2026.7.4] — 2026-07-08

v2026.7.4 is a provisioning reliability, session durability, provider-loadout,
reporting, and structured-output release. It bounds libvirt CLI calls during VM
provisioning, preserves tmux-backed sessions across agent restarts, closes
provider CLI install drift, backfills monthly reports to project inception, and
adds a structured agent-output SSE stream for Cockpit Chat projections. See
[`docs/releases/v2026.7.4.md`](docs/releases/v2026.7.4.md).

### Added

- **Structured agent-output SSE stream** (#600): `GET
  /api/v1/agent-output/stream` now emits `agentic.agent_output.v1` JSON events
  with `agent_id`, `command_id`, stream kind, timestamp, base64 raw bytes, and
  readable text projection. The stream supports `agent_id`, `command_id`,
  `stream`, bounded `replay`, and `limit` filters.

- **Monthly report backfill** (#601): added January through May 2026 monthly
  reports and a monthly index under `.aiwg/reports/`, with evidence and
  carryover notes for each backfilled month.

### Fixed

- **Bounded libvirt calls in VM provisioning** (#614): `provision-vm.sh` and
  libvirt helper paths now route `virsh` through a timeout wrapper controlled by
  `AGENTIC_VIRSH_TIMEOUT_SECONDS`, preventing wedged libvirtd calls from
  leaving provisioning operations running forever.
- **Session survival across agent restarts** (#613): agent restart cleanup no
  longer kills tmux servers, systemd units use `KillMode=process`, and the
  agent can adopt existing tmux sessions into session reports/reconciliation.
- **Provider loadout install parity** (#612): loadout regression coverage now
  proves the `verify-providers` profile installs every non-artifact provider
  CLI, including Codex, Copilot, Cursor, Factory, OpenCode, OpenClaw, and
  Claude.

### Documentation

- Documented the structured agent-output SSE endpoint in the REST API docs.
- Backfilled AIWG monthly reports from 2026-01 through 2026-05.

### Operator notes

- Operators can tune libvirt command timeouts with
  `AGENTIC_VIRSH_TIMEOUT_SECONDS`; the default is 15 seconds.
- Agent service restarts no longer terminate tmux-backed user sessions in the
  service cgroup. Set `AGENTIC_ADOPT_TMUX_SESSIONS=0` to disable tmux session
  adoption if a deployment needs the previous fail-closed behavior.
- Cockpit Chat and other projection clients should prefer
  `/api/v1/agent-output/stream` over the legacy WebSocket output projection
  when they need byte-preserving structured output.

## [2026.7.3] — 2026-07-07

v2026.7.3 is an admin-v2 VM lifecycle, runtime enrollment, and host-session
listing fix release. It keeps stopped admin-provisioned VMs visible after agent
disconnect, makes destroy release libvirt/storage/IP/CID state, restores
host-runtime PTY sessions in the formal session-list API, and fixes host and
container enrollment routing found during Cockpit UAT. See
[`docs/releases/v2026.7.3.md`](docs/releases/v2026.7.3.md).

### Fixed

- **Stopped VM inventory retention** (#607): admin-v2 qemu provisions now
  persist the libvirt launch/domain name in `InstanceContext`, and
  `GET /api/v2/admin/instances` uses that mapping so stopped `cockpit-*` VMs
  remain visible after the in-guest agent disconnects.
- **Complete VM destroy cleanup** (#608): admin-v2 destroy now resolves
  stopped or disconnected VMs through the persisted launch name, undefines the
  libvirt domain, removes the VM storage directory, and releases `.ip-registry`
  and `.vsock-cid-registry` allocations.
- **Host runtime session listing** (#611): `SessionReport` handling imports
  agent-reported live PTY sessions into dispatcher inventory before
  reconciliation, so `GET /api/v1/agents/{id}/sessions` returns host/local
  supervisor sessions.
- **Host runtime mTLS enrollment** (#609): local host agents now default to the
  mTLS gRPC listener when mTLS is configured, while preserving
  `AGENTIC_HOST_GRPC_SERVER` as an override.
- **Container bootstrap reachability** (#610): the HTTP bootstrap/dashboard
  listener can be widened with `AGENTIC_HTTP_LISTEN_IP` for Docker
  `host.docker.internal` enrollment paths while retaining loopback defaults.

### Operator notes

- Operators can stop a VM and still start or destroy it from admin-v2
  inventory.
- Destroying VM instances now releases static IP and VSock CID allocations;
  stale entries left by earlier releases may still require one-time manual
  cleanup.
- Host runtime Cockpit/session clients no longer need to synthesize a session
  row when the API returns agent-reported sessions.
- Container bootstrap deployments that need host-gateway access should set
  `AGENTIC_HTTP_LISTEN_IP=0.0.0.0` or another reachable bind address explicitly.

## [2026.7.2] — 2026-07-06

v2026.7.2 is the Observe/Drive controller-lease and reconnect-metadata
release. It makes terminal control authority explicit across the formal session
registry, REST/WS session lists, CLI, and dashboard so reconnecting clients can
distinguish controller and observer state before sending input. See
[`docs/releases/v2026.7.2.md`](docs/releases/v2026.7.2.md).

### Added

- **Agent-scoped session attach metadata**: `GET /api/v1/agents/{id}/sessions`
  now returns `pty_ws_url`, `pty_ws_subprotocol`, orchestrator observer and
  controller URLs, default role, controller policy, membership, and liveness
  fields for each listed session.
- **Session-list liveness and membership parity**: REST and WebSocket session
  list surfaces now expose controller IDs, observer IDs, attachment counts,
  replay sequence state, and maximum client lag in a consistent shape.
- **Dashboard reconnect metadata**: existing session cards show controller
  lease state, observer counts, and replay sequence state, and v2 attach uses
  the listed PTY URL when present.

### Fixed

- **Formal controller lease enforcement**: controller authority is now a
  singleton lease in the formal session registry. Additional controller attach
  requests are downgraded to observer until the live controller detaches or its
  closed channel is reaped.
- **Fresh observer replay fallback**: fresh joins replay from the newest
  retained keyframe when available, or from the oldest retained ring frame when
  no keyframe exists yet, preventing late observers from starting on a blank
  terminal.
- **Idempotent interactive session creation**: successful agent-scoped session
  creation responses can be cached with `Idempotency-Key`, avoiding duplicate
  terminal sessions after client retry or network timeout.
- **CLI read-only downgrade handling**: `sandboxctl session attach
  --controller` now honors the server-granted role and suppresses stdin when a
  controller request is downgraded to observer.

### Documentation

- Updated the REST API, WebSocket protocol, and TUI orchestration docs with the
  singleton controller lease, session-list metadata, replay fallback, and
  idempotent create semantics.
- Recorded the 2026-07-06 release code-to-docs audit under `.aiwg/reports/`.

### Operator notes

- Existing clients should trust the `RoleAssigned` or listed membership state
  rather than assuming a requested controller attach can write.
- External terminal clients should prefer the listed `pty_ws_url` and
  `pty_ws_subprotocol` for reconnects instead of constructing attach URLs from
  static templates.

## [2026.7.1] — 2026-07-02

v2026.7.1 is a release-publication recovery cut for v2026.7.0. It keeps the
July runtime hardening payload unchanged and lowers release-artifact Cargo
parallelism so the tag workflow can build x86_64 management tarballs and Linux
packages without overcommitting the shared Gitea/act runner. See
[`docs/releases/v2026.7.1.md`](docs/releases/v2026.7.1.md).

### Fixed

- **Release artifact runner pressure**: release tarball and Linux package jobs
  now use conservative Cargo fan-out during tag workflows. The previous
  v2026.7.0 tag validated code, Docker, and live E2E, but the duplicated
  x86_64 management/package artifact builds were killed without retrievable job
  logs while Docker/E2E/release builds ran concurrently.

### Changed

- **Release roll-forward**: v2026.7.1 supersedes the failed v2026.7.0
  publication attempt. The product/runtime changes remain those documented for
  v2026.7.0, with this cut adding the release CI correction needed to publish
  artifacts.

### Operator notes

- Use v2026.7.1 as the July release tag. v2026.7.0 was pushed, but its release
  artifact workflow failed before release attachment.

## [2026.7.0] — 2026-07-02

v2026.7.0 is the July runtime hardening and release-readiness cut. It keeps the
v2026.6.36 release surface, adds stronger credential-proxy abuse controls and
leakage evidence, makes QEMU provisioning tolerate first-boot shutdown behavior,
raises management startup file-descriptor resilience for dev/transient launches,
and promotes the new documentation/blog surfaces. See
[`docs/releases/v2026.7.0.md`](docs/releases/v2026.7.0.md).

### Added

- **Credential leakage harness** (#518): added a regression harness that proves
  managed credential APIs and proxy responses redact active secret material from
  metadata, lease, denied, rate-limited, and proxied-response paths.
- **QEMU first-boot restart coverage** (#597): added provisioning logic and
  shell regression coverage for guests that intentionally power off during their
  first boot customization window.
- **Project blog docs surface**: added the first project blog article and wired
  the blog section into the docs manifest and welcome page.

### Security

- **Credential proxy rate limiting** (#596): lease proxy policy now supports
  per-minute rate limits scoped by active lease plus session identity, returns
  `429` with `Retry-After`, and denies expired or revoked leases before any
  accounting.
- **Credential proxy redaction hardening**: HTTP proxy responses redact injected
  secret material from upstream headers and bodies before returning data to the
  workload.

### Fixed

- **Management file-descriptor ceiling**: management now raises its soft
  `RLIMIT_NOFILE` to the available hard limit at startup, preventing
  dev/transient launches with a low soft cap from exhausting libvirt sockets
  during repeated inventory and health checks.
- **QEMU first-boot provisioning** (#597): `images/qemu/provision-vm.sh`
  observes the first-boot window and restarts guests that shut off before the
  runtime wait phase.
- **Docsite terminal viewport**: the terminal frame is anchored to the viewport
  so the docs shell remains usable while navigating the site.

### Changed

- **Issue audit state** (#503, #507, #518, #597): refreshed open-issue audit
  evidence after the July live-validation pass and constrained claims where
  live agent/PTY or direct egress-bypass evidence remains outstanding.

### Documentation

- Updated credential-proxy, ASVS, attack-surface, transport-security, and
  security-status docs with the implemented proxy rate-limit/redaction controls
  and remaining egress-proof limitations.
- Updated release verification docs and recorded the 2026-07-02 code-to-docs
  audit under `.aiwg/reports/`.

### Operator notes

- Operators using the credential proxy can configure per-lease HTTP proxy
  limits through `proxy_policy.rate_limit_per_minute`. Direct upstream network
  egress controls are still required when the workload can reach a provider
  outside the proxy path.
- Operators launching management outside the packaged systemd unit get the same
  startup attempt to raise the process file-descriptor soft limit, provided the
  inherited hard limit permits it.

## [2026.6.36] — 2026-06-29

v2026.6.36 is a release-publication recovery cut for v2026.6.35. It keeps the
Observe/Drive runtime payload unchanged, adds retry hardening to the public
container mirror job, and republishes the Linux packages, installer assets, and
GHCR image matrix from a clean tag workflow. See
[`docs/releases/v2026.6.36.md`](docs/releases/v2026.6.36.md).

### Fixed

- **Public container mirror retry**: the GHCR/Quay mirror step now retries
  transient `docker pull` and `docker push` failures with exponential backoff,
  covering registry-side `unknown blob` failures observed while publishing the
  large `agentic-sandbox-agent` image for v2026.6.35.

### Changed

- **Release supersedence**: v2026.6.36 supersedes v2026.6.35 for consumers who
  require the full public container image matrix. The product/runtime payload is
  otherwise the same Observe/Drive reliability release content.

### Operator notes

- Prefer v2026.6.36 over v2026.6.35 for new installs and upgrades so package,
  installer, and public container publication all come from the same successful
  tag workflow.
- Operators who already installed v2026.6.35 from Linux packages do not need a
  runtime rollback; v2026.6.36 exists to complete the publication surface and
  preserve a clean release audit trail.

## [2026.6.35] — 2026-06-29

v2026.6.35 is the Observe/Drive reliability release across the live terminal
tiers. It keeps the v2026.6.34 QEMU/vsock baseline, adds the HTTP credential
proxy backend for scoped API access, repairs pty-ws bridge output and stale
controller cleanup, and records the release code-to-docs sync. See
[`docs/releases/v2026.6.35.md`](docs/releases/v2026.6.35.md).

### Added

- **HTTP credential proxy backend**: added
  `POST /api/v2/credential-proxy/http`, allowing managed sessions to invoke
  approved upstream HTTP/API targets through an active credential lease without
  exposing the secret to the workload.

### Fixed

- **Live Observe/Drive output path** (#594): real pty-ws bridge output now stays
  connected to the canonical event stream for VM sessions, so controller and
  observer clients continue receiving live terminal output instead of falling
  silent after the bridge handoff.
- **Stale pty-ws controller slots** (#598): the executor now sends WebSocket
  heartbeat Pings, reaps clients that stop answering Pong frames, and runs the
  existing detach cleanup so an idle or half-open controller socket cannot leave
  the session permanently stuck as observer/`pty.permission_denied`.
- **vsock CID and teardown cleanup**: QEMU vsock CID ownership, teardown
  cleanup, and E2E VM reaping paths were hardened so stale registry and cleanup
  state do not leak across provision/destroy cycles.

### Changed

- **Current release matrix**: Darwin/macOS artifacts are deferred from the
  public release asset gate while Linux packages, installer assets, and GHCR
  images remain the active release surface.

### Documentation

- Synced the pty-ws contract and reconnect example with the implemented
  heartbeat/stale-controller reap behavior.
- Recorded the 2026-06-29 release code-to-docs audit under `.aiwg/reports/`.
- Added credential-proxy API and security documentation for the new HTTP proxy
  backend.

### Operator notes

- Operators relying on live terminal control should upgrade both management and
  executor binaries so the bridge output and stale-controller cleanup fixes land
  together.
- The pty-ws heartbeat is server-side and does not require browser clients to
  send application-level keepalives; compliant WebSocket clients only need to
  answer standard Ping control frames.
- macOS/Darwin release assets are intentionally not part of this cut's required
  publication proof.

## [2026.6.34] — 2026-06-28

v2026.6.34 continues the QEMU/vsock transport hardening line from
v2026.6.31–.33. It repairs several vsock and VM-lifecycle edge cases surfaced
after the v2026.6.33 cut and makes the base-image bake survive hosts with a
root-only `/boot`. The runtime surface is unchanged from v2026.6.33. See
[`docs/releases/v2026.6.34.md`](docs/releases/v2026.6.34.md).

### Fixed

- **vsock CID registry lock ownership** (#588): the per-VM CID registry lock is
  created and owned correctly, preventing lock-contention failures during
  concurrent provision/destroy.
- **VM destroy cleanup trap scope** (#590): the destroy cleanup trap stays in
  scope so teardown reliably releases the DHCP reservation, vsock CID, and
  ephemeral key allocations.
- **File-backed CID maps on dev startup** (#589): `management/dev.sh` honors a
  file-backed vsock CID map (`AGENTIC_GRPC_VSOCK_CID_MAP_FILE`) at startup,
  matching the runtime transport identity resolver.
- **vsock-only guest enrollment**: provisioned QEMU guests enroll over vsock
  without requiring a loopback-reachable network path back to management.
- **TLS server-name override** (host-runtime): the host runtime accepts an
  explicit TLS server-name override for its mTLS connections.
- **libguestfs base-image bake fallback** (#592):
  `images/qemu/build-base-image.sh` escalates the bake through
  `LIBGUESTFS_BACKEND=direct` and then `sudo -E` when supermin cannot read a
  root-only `/boot`, so the agent-baked image builds on restricted-`/boot`
  hosts. Non-regressive: the default backend runs first.

### Documentation

- Recorded the 2026-06-24 open-issue audit and follow-up under `.aiwg/reports/`.

## [2026.6.33] - 2026-06-26

v2026.6.33 is the follow-up release-flow cut for the QEMU/vsock line. It
keeps the v2026.6.32 runtime surface, then adds the release-runner hardening
needed to keep image provenance and publication mirrors reproducible. See
[`docs/releases/v2026.6.33.md`](docs/releases/v2026.6.33.md).

### Fixed

- The QEMU base-image builder now has an explicit automation-safe overwrite
  path (`--yes`/`--force`) and refuses non-interactive replacement of an
  existing image unless that intent is explicit (#585).
- The base-image build fails early when `/mnt/ops/base-images` cannot be
  written, surfacing the required operator remediation before the expensive
  guest bake starts (#585).
- `images/qemu/build-base-image.sh` is source-safe, so script tests can load
  and exercise guard behavior without accidentally starting a full image build
  (#585).
- Release mirror sidecar containers now use YAML-valid volume/list syntax in
  the tag workflow path, preserving the v2026.6.32 release publication lane.

### Tests

- Added shell regression coverage for base-image overwrite refusal, explicit
  force overwrite, writable directory creation, unwritable directory fail-fast,
  and pre-ISO failure behavior (#585).
- Branch CI run `1793` passed on `e0de9f8`, including Docker publish, security
  scan, and E2E with `6 passed; 0 failed`.

### Operator notes

- The grissom E2E runner was repaired by replacing its stale
  `/mnt/ops/base-images/ubuntu-server-24.04-agent.qcow2` and manifest with the
  verified titan image pair:
  `a8d2b97b14eb45215f1f333ec2f4ed7dae751c217b07e3c59aae9fac374008c1`.
- CI image/manifest drift should be fixed by rebuilding with
  `images/qemu/build-base-image.sh 24.04 --yes` on the affected runner or by
  installing a verified image/manifest pair from titan.

## [2026.6.32] - 2026-06-25

v2026.6.32 is the release-flow completion for the QEMU/vsock transport line.
It keeps the v2026.6.31 runtime surface, fixes the base-image bake and CI
acceptance path that blocked publication, and records the live E2E proof for
#561. See [`docs/releases/v2026.6.32.md`](docs/releases/v2026.6.32.md).

### Fixed

- `management/dev.sh` now accepts the documented
  `AGENTIC_GRPC_VSOCK_CID_MAP_FILE` source when deciding whether the host vsock
  listener can start, and exports the file path so the spawned
  `agentic-mgmt` inherits it (#584).
- The QEMU base-image build no longer lets the first-boot post-install service
  build `agent-client` inside the guest during the image bake; the host build
  remains the single source of the baked binary (#561).
- The unattended base-image bake waits for the guest boot/install path to
  settle and verifies the baked `agent-client`, vsock module, and manifest
  provenance before accepting the image (#561).
- CI now fails fast when the runner-local QEMU base image and manifest drift
  apart, preventing stale runner state from masquerading as an E2E failure
  later in the job (#561).

### Tests

- Full manual CI run `1776` completed successfully on
  `18f9336f8a96bee971ee40972e79f79a133cf447`, including Docker publish and
  E2E.
- E2E job `57383` passed with `6 passed; 0 failed`, clearing the #561 live
  acceptance gate.

### Operator notes

- Runner-local base-image state matters for E2E. The failed run was caused by
  stale image/manifest state on the grissom runner, not by titan's canonical
  image. Grissom was repaired with the titan-good image manifest:
  `a8d2b97b14eb45215f1f333ec2f4ed7dae751c217b07e3c59aae9fac374008c1`.

## [2026.6.31] — 2026-06-24

Same-host QEMU VM transport now uses gRPC over `AF_VSOCK` (ADR-023/ADR-026),
resolving the #561 enrollment failure where qemu VMs reached `running` but the
in-guest agent never enrolled (stuck `bootstrap-pending`). See
[`docs/releases/v2026.6.31.md`](docs/releases/v2026.6.31.md).

### Added

- Per-VM vsock CID allocation with libvirt `<vsock>` device injection, recorded
  in `vm-info.json`; cloud-init emits and recognizes the
  `AGENT_GRPC_VSOCK_CID/PORT` transport tuple (#571, #569, #570).
- Host-side vsock CID identity lifecycle: register on provision, unregister on
  destroy, startup validation of `AGENTIC_GRPC_VSOCK_CID_MAP`, and `SIGHUP`
  reload of `AGENTIC_GRPC_VSOCK_CID_MAP_FILE` with an atomic swap (#574, #577,
  #583).
- `flock`-serialized CID registry to keep parallel provisioning collision-free
  (#581); destroy/reap cleanup and `vm-info.json` reconciliation (#575, #579).
- Base image bakes the `vmw_vsock_virtio_transport` module plus `socat`/
  `iproute2` and verifies them at build time (#578).

### Fixed

- v2 admin lifecycle ops resolve the libvirt domain by mapping the instance_id
  to candidate domain names instead of the raw instance_id (which never matched
  qemu domains); idempotent destroy is re-gated on a correct lookup (#563).
- Standardized the in-guest `agent-client` path on `/opt/agentic-sandbox/bin/agent-client`
  across the base image, live-deploy, and provisioning readiness check (#573).

### Documentation

- Synced gateway-mediated SSH docs with the landed certificate lease API
  (`/api/v2/gateway/ssh/leases`); added a deployment section and the
  `AGENTIC_GATEWAY_SSH_*` env reference (ADR-029, #530/#531/#532).
- Documented the vsock CID registry layout, map reload, and teardown signaling
  (#580, #582).

### Tests

- Real-libvirt unit tests are serialized on a shared lock to remove parallel
  flakiness; added e2e secure-transport, vsock CID lifecycle, and agent-client
  path-parity script suites (#572).

### Operator notes

- Enable the host vsock listener with `AGENTIC_GRPC_VSOCK_PORT` +
  `AGENTIC_GRPC_VSOCK_CID_MAP`; rebuild the QEMU base image
  (`images/qemu/build-base-image.sh`) to pick up the baked vsock module and the
  current `agent-client`. The container tier is unaffected.

## [2026.6.30] — 2026-06-23

### Added

- Admin v2 `POST /api/v2/admin/instances` accepts an optional `ssh_key`; the
  qemu path forwards it to `provision-vm.sh` as `--ssh-key`, reaching parity
  with the v1 provisioning path (#558).
- `GET /healthz/deep` reports `host_runtime_enabled`, so conformance/UAT
  harnesses can treat the opt-in host tier as skipped rather than failed when
  no supervisor is configured (#555).
- Dedicated `Host Runtime` CI tier (`.gitea/workflows/host-runtime.yml`) boots
  with the local supervisor enabled and asserts `host_runtime_enabled: true`
  and that host provisioning is accepted (202), not fail-closed (501) (#555).

### Fixed

- VM provisioning no longer fails with `No SSH public key found. Specify with
  --ssh-key` when the caller (e.g. the AIWG Cockpit bridge) supplies a key —
  the v2 `ProvisionRequest` previously dropped `ssh_key` silently (#558).
- The host-runtime `501 runtime.not_implemented` detail now points at
  `AGENTIC_HOST_RUNTIME_ENABLED=1` and `docs/runtimes/host-supervisor.md`
  instead of citing the closed host-target issue as a pending blocker (#555).

### Documentation

- `docs/contracts/admin-api.openapi.yaml` documents the v2 provision `ssh_key`
  field; `docs/runtimes/host-supervisor.md` documents the `host_runtime_enabled`
  capability signal and the Host Runtime CI tier.

## [2026.6.29] — 2026-06-22

### Added

- Published OWASP ASVS / Top 10 API security profile, standards-alignment
  matrix, security status page, release artifact verification guide, and
  attack-informed test catalog.
- VM provisioning now records base image, cloud-init seed ISO, and loadout
  manifest provenance hashes in VM metadata.

### Fixed

- Local CA gRPC mTLS dev material now rotates stale server leaves so refreshed
  roots do not leave old server certificates in place.
- The dashboard serves embedded assets with a CSP that disallows inline
  scripts, and representative DOM sinks are covered by a sentinel regression.
- Docker-reachable dev launch now fails before management starts unless
  `AGENTIC_ALLOW_PLAINTEXT_TCP=1` acknowledges non-loopback plaintext exposure.
- Admin v2 Docker inventory now reports `agent_registered`, `agent_ready`, and
  `container_finished_at`; exited containers without a registered agent surface
  as `operation_status: "not_ready"` instead of provisioned/ready.
- Admin v2 Docker provisioned `InstanceContext` rows stay unready until an
  actual Ready heartbeat marks them routable.
- Admin v2 QEMU provisioning resolves `images/qemu/provision-vm.sh` from the
  stable checkout root, supports `AIWG_PROVISION_VM_SCRIPT`, and reports all
  attempted paths on spawn failure.
- CI now documents the accepted titan runner posture, and VM provisioning
  provenance is recorded in release verification docs.

### Documentation

- Updated getting-started and management README Docker dev recipes with the
  explicit plaintext acknowledgement and mTLS control-stream posture.
- Updated the admin v2 OpenAPI contract, container runtime guide, and TUI
  support runbook with Docker readiness fields and not-ready semantics.
- Refreshed security posture docs so CSP/DOM-sink hardening is current while
  remote multi-user dashboard hardening remains unclaimed.
- Added `.aiwg/reports/doc-sync-audit-2026-06-22-release.md` for the
  code-to-docs release gate and `.aiwg/reports/open-issue-audit-2026-06-22.md`
  for release blocker/backlog status.

### Verification

- `bash -n management/dev.sh`
- `cargo test provision_vm_script --lib`
- `cargo test provision_vm_spawn_error --lib`
- `cargo test docker --lib`
- `cargo test ready_heartbeat_marks_preregistered_context_ready_without_replacing_it --lib`
- `cargo fmt --check`
- `git diff --check`

### Operator notes

- Docker-agent dev mode now requires a deliberate
  `AGENTIC_ALLOW_PLAINTEXT_TCP=1` acknowledgement when binding management
  outside loopback for container bootstrap.
- Cockpit and other bridge consumers should wait for `agent_ready: true`
  instead of treating runtime inventory or AgentCard metadata as session
  readiness.

## [2026.6.28] — 2026-06-22

### Added

- Gateway-mediated SSH access is now implemented end-to-end: lease metadata API,
  OpenSSH certificate signing, runtime trust provisioning, connector routing,
  CLI UX, and regression coverage for the gateway SSH path.
- `pty-ws` sessions now converge on the formal session registry and canonical
  session bus for replay, input, resize, signal, and membership state.

### Fixed

- SSH connector routing now fails closed unless
  `AGENTIC_GATEWAY_SSH_ALLOWLIST` grants an actor-to-instance route, removing
  the previous implicit trust path for gateway SSH attachments.
- SSH lease issuance and revocation now require an authenticated operator
  identity and bind the lease actor to that identity instead of trusting request
  bodies.
- SSH lease revocation responses now expose the implemented certificate
  semantics as `metadata_only_until_certificate_expiry`, making the short-lived
  certificate window explicit to API clients and operators.
- SSH connector prelude parsing is bounded, so malformed clients cannot force
  unbounded memory growth by withholding the newline delimiter.
- Legacy direct AIWG SSH proxy examples and legacy direct-runtime SSH key
  rotation are now scoped as bypass/legacy paths, distinct from
  gateway-mediated SSH.
- `pty-ws` terminal transport fixes that landed after the `v2026.6.27` tag are
  included in this release: legacy wildcard fanout stays disabled by default,
  canonical replay cursor delegation is covered, and formal bus projection is
  exercised by tests.
- The SSH handshake failure fixture now deterministically waits for the proxied
  client banner before closing the upstream, removing a race in branch CI.

### Documentation

- Accepted ADR-029 for gateway-mediated SSH terminal access and added the
  SSH non-exclusivity spike so public terminal-access claims distinguish SSH,
  `pty-ws`, and direct-runtime SSH.
- Updated public API documentation for SSH lease authorization, lease
  revocation semantics, connector allowlist configuration, and bounded
  connector prelude behavior.
- Recorded the SSH gateway release documentation sync at
  `.aiwg/reports/doc-sync-audit-2026-06-22.md`.

### Verification

- Gitea Actions run `1619` completed successfully on `8350096`, including
  lint, tests, build, Docker build/publish, E2E tests, and security scan.
- Gitea Actions conformance run `1620` completed successfully on `8350096`.
- `make test-unit`
- `make lint`
- `git diff --check`

### Operator notes

- Set `AGENTIC_GATEWAY_SSH_ALLOWLIST` before enabling the SSH connector
  listener. Routes use explicit actor-to-instance bindings; absent or malformed
  rules deny SSH attachments.
- SSH lease revocation updates gateway metadata immediately, but already issued
  OpenSSH user certificates remain usable until their short TTL expires.
- Prefer gateway-mediated SSH for standards-compatible operator access. Direct
  runtime SSH remains a bypass/break-glass path and does not carry gateway
  authorization or audit guarantees.

## [2026.6.27] — 2026-06-21

### Added

- `pty-ws` now supports a binary hot path via the `pty-ws.v1.binary`
  subprotocol. Hot PTY output uses `PW1O` binary frames, hot input uses `PW1I`
  binary frames, and JSON/base64 replay remains available for compatibility.
- `pty-ws` attach authorization now supports observe, control, and admin
  scopes. Observer-scoped attaches can replay and watch sessions but cannot
  write input, resize the PTY, or claim controller authority.
- Externally owned `pty-ws` sessions now register with the formal session
  registry so inventory, replay, and formal input/resize/signal paths work
  through the canonical session bus.

### Fixed

- PTY sessions now emit exactly one retained `Closed` frame for command
  results, bridge EOF, bridge start failure, last-member leave, and management
  teardown. Command-result closes carry the agent exit code with
  `reason: "command_result"`.
- Legacy management WebSocket wildcard terminal fanout
  (`agent_id="*"`) is disabled by default and now requires the explicit
  `AGENTIC_WS_ALLOW_WILDCARD_SUBSCRIBE=true` operator opt-in.
- Docker-backed admin inventory and agentshare session paths now preserve the
  Docker runtime metadata and mounted workspace visibility landed after
  `v2026.6.26`.

### Documentation

- Updated the `pty-ws/v1` and `pty-extensions/v1` contract specs for binary
  frames, attach scopes, deterministic close frames, single-controller
  reference-profile terminology, and conformance coverage.
- Updated the management WebSocket protocol docs to mark legacy wildcard
  subscriptions as deprecated and disabled by default.
- Recorded the release code-to-docs audit at
  `.aiwg/reports/doc-sync-audit-2026-06-21.md`.

### Verification

- `make test`
- `bash -n scripts/bump-version.sh`
- `bash -n scripts/run-e2e-tests.sh`
- `bash -n scripts/verify-release-assets.sh`
- `scripts/lint-ci-pins.sh`
- `scripts/lint-npm-pins.sh`
- `git diff --check`
- `cargo fmt --manifest-path management/Cargo.toml --check`
- `cargo fmt --manifest-path agent-rs/Cargo.toml --check`
- `cargo fmt --manifest-path cli/Cargo.toml --check`
- `cargo test --manifest-path management/Cargo.toml --lib`
- `cargo test --manifest-path agent-rs/Cargo.toml --lib`
- `cargo test --manifest-path cli/Cargo.toml --bins`
- `python3 scripts/check-doc-links.py --docs-root docs`

### Operator notes

- Use `v2026.6.27` for the terminal transport hardening release that adds
  binary `pty-ws`, scoped PTY attach authorization, formal session bus
  registration, deterministic close frames, and the legacy wildcard terminal
  broadcast restriction.
- Existing JSON/base64 `pty-ws.v1` clients remain supported. New high-throughput
  terminal clients should negotiate `pty-ws.v1.binary`.

## [2026.6.26] — 2026-06-20

### Fixed

- Release workflow now builds the `aarch64-unknown-linux-gnu` `agent-rs` and
  `cli` artifacts on the Linux runner with the GNU aarch64 cross toolchain,
  leaving mutsu responsible only for the Apple Silicon Darwin artifact. This
  avoids blocking release publication on a stale mutsu Linux matrix leg after
  artifact upload.
- Renamed the Linux release artifact job so the Actions UI no longer labels the
  ARM64 Linux matrix entry as x86_64-only.

### Verification

- Gitea Actions runs `1502`, `1503`, and `1504` completed successfully on
  `9760108`.
- Local `aarch64-unknown-linux-gnu` release builds produced ARM64 Linux ELF
  binaries for `agent-client` and `sandboxctl` using
  `gcc-aarch64-linux-gnu` plus the target libc development sysroot.

## [2026.6.25] — 2026-06-20

### Fixed

- Supersedes `v2026.6.24` for the container/host bootstrap quick path. The
  previous tag packaged the Docker provider helper fixes, but bootstrap-enrolled
  agents still needed an explicit release gate proving the static-cert gRPC
  mTLS listener accepts CSR-issued SPIFFE client leaves and authorizes their
  peer identity through `AgentService::connect`.
- Added static-cert gRPC mTLS regressions covering both raw rustls acceptance
  and the full tonic `connect` RPC path for bootstrap CSR-issued client
  certificates.
- `GET /` now returns the management service health payload instead of falling
  through to the embedded UI/static fallback, giving load balancers and simple
  HTTP probes a stable root health signal.
- Admin v2 instance objects now expose connected-agent transport posture:
  `transport`, `transport_posture`, structured `security_posture`, and
  host-runtime `host_daemon` status. Cockpit and other bridge consumers no
  longer have to render host-backed instances as `Unknown transport (unknown)`
  when the runtime registered over authenticated mTLS.
- `agent-client` now logs the full error cause chain for connect and stream
  failures, so TLS alerts and tonic transport errors are visible instead of
  being collapsed to only `Failed to connect to management server over mTLS`.

### Documentation

- Added the dated terminal-transport benchmark harness and artifacts for issue
  #520, qualifying gRPC PTY, binary `pty-ws`, SSH, SSH ControlMaster, tmux,
  Mosh, ttyd/GoTTY, and Kubernetes-style exec claims with repeatable simulated
  baseline data.
- Added ADR/planning documentation for gateway-mediated SSH access as a
  first-class option with different semantics from `pty-ws`, not an unmanaged
  fallback or replacement for the session bus.
- Added launch security posture, credential posture, and attack-surface
  inventory documents so release claims distinguish implemented controls from
  qualified or deferred security work.
- Documented the static mTLS listener peer-identity contract for bootstrap
  enrollment, including the `x-agent-instance-id` match requirement.
- Updated development bootstrap guidance to call out that container enrollment
  needs a Docker-reachable HTTP bootstrap URL in addition to the Docker-reachable
  mTLS listener.
- Updated the admin v2 OpenAPI contract with the new transport posture and
  host-daemon fields.

### Verification

- `cargo test 'grpc_mtls_static' --manifest-path management/Cargo.toml`
- `cargo test --manifest-path management/Cargo.toml --bin agentic-mgmt`
- `cargo test --manifest-path agent-rs/Cargo.toml`
- `cargo test --lib` from `management/` after the admin-v2 transport posture
  change: 661 passed, 0 failed.
- `python3 -m py_compile scripts/benchmark-terminal-transports.py`
- `python3 scripts/benchmark-terminal-transports.py --out-dir .aiwg/testing --prefix terminal-transport-benchmark-2026-06-19`
- Live isolated container smoke on high ports: bootstrap enrollment materialized
  mTLS credentials, rustls reached client auth, the agent connected over mTLS,
  the server logged the bootstrap SPIFFE peer identity, registration succeeded,
  and metrics continued over the stream.

### Operator notes

- Use `v2026.6.25` for bootstrap mTLS peer-identity diagnostics plus the
  admin-v2 transport posture fields needed by Cockpit and other fleet bridges.
- Terminal performance claims remain qualified: the release includes a
  repeatable simulated benchmark harness and raw artifacts, but fixture-backed
  runs are still required before stronger faster/lighter-than-SSH language.

## [2026.6.24] — 2026-06-19

### Fixed

- Supersedes `v2026.6.23`. The `v2026.6.23` tag carried the Docker/VM runtime
  bootstrap and SSH-readiness fixes, but it did not package the normalized
  Claude provider discovery/readiness/launch helpers into the Docker images
  needed for a real managed Claude session proof.
- `agentic/claude:latest` now ships `agentic-provider-inventory`,
  `agentic-provider-readiness`, and `agentic-claude-automation`, matching the
  VM automation-control helper contract.
- `agentic/automation-control:latest` now ships the missing
  `agentic-provider-readiness` and `agentic-claude-automation` helper scripts,
  so orchestrators can probe and route Claude capability consistently even when
  the Codex-based control image reports the Claude CLI as absent.
- Container smoke tests now verify the Claude helper contract and provider
  readiness schema so the release gate catches image/helper drift.

### Operator notes

- Use `v2026.6.24` for the Docker/VM runtime bootstrap injection and Claude
  provider-session packaging release. Treat `v2026.6.20` through
  `v2026.6.23` as superseded release-attempt tags.

## [2026.6.23] — 2026-06-19

> Superseded by `v2026.6.24`. The tag carried the Docker/VM bootstrap and
> SSH-readiness fixes, but the Docker provider images still lacked the
> normalized Claude control helpers required for the requested live managed
> Claude session path.

### Fixed

- Supersedes `v2026.6.22`. The `v2026.6.22` tag carried the Docker/VM runtime
  bootstrap injection fix and passed local verification, lint, unit tests,
  build, package, and Docker publish jobs, but tag CI failed when the VM E2E
  resource-stress test hit a transient SSH readiness gap on the shared VM.
- VM-backed Rust E2E target discovery now waits for SSH readiness instead of
  failing on a single transient probe. The wait defaults to 60 seconds and can
  be tuned with `AGENTIC_RUST_VM_E2E_SSH_READY_SECONDS`.

### Operator notes

- Use `v2026.6.24` for the Docker/VM runtime bootstrap injection and Claude
  provider-session packaging release. Treat `v2026.6.20` through
  `v2026.6.23` as superseded release-attempt tags.

## [2026.6.22] — 2026-06-19

> Superseded by `v2026.6.23`. The tag carried the fix and passed local
> verification plus Gitea lint/test/build/package/Docker publish jobs, but the
> VM E2E resource-stress test failed on a transient SSH readiness gap before
> release creation.

### Fixed

- Supersedes `v2026.6.21`. The `v2026.6.21` tag carried the normalized runtime
  bootstrap injection fix and passed local lint/test plus the Gitea test and
  security jobs, but the Gitea lint job failed without a retrievable log file
  and this Gitea version does not expose a workflow rerun endpoint.
- Re-tags the Docker/VM runtime bootstrap injection release on a fresh release
  version so the release pipeline can run from a clean tag event.
- Added narrow test preflight guards for local Unix-socket and loopback bind
  denial so restricted developer sandboxes report the environment limitation
  without masking real bind or TLS failures in normal CI.

### Operator notes

- Use `v2026.6.22` for the Docker/VM runtime bootstrap injection release.
  Treat `v2026.6.20` and `v2026.6.21` as superseded release-attempt tags.

## [2026.6.21] — 2026-06-19

> Superseded by `v2026.6.22`. The tag carried the fix and passed local
> verification plus the Gitea test/security jobs, but the Gitea lint job failed
> without a retrievable log file and could not be rerun through the installed
> Gitea API.

### Fixed

- Supersedes `v2026.6.20`. The `v2026.6.20` tag carried the normalized runtime
  bootstrap injection fix but its tag CI failed because the new fake-Docker
  regression test was not safe under the full parallel management test suite.
- Made the fake-Docker regression test append command invocations and return
  command-specific output, so concurrent Docker-list tests cannot overwrite the
  captured `docker run` arguments.

### Operator notes

- Use `v2026.6.21` for the Docker/VM runtime bootstrap injection release.
  Treat `v2026.6.20` as a superseded release-attempt tag.

## [2026.6.20] — 2026-06-19

> Superseded by `v2026.6.21`. The tag was pushed but failed the tag CI test
> job because the fake-Docker regression test was not parallel-safe.

### Fixed

- Normalized runtime bootstrap enrollment injection across host, Docker, and
  VM provisioning. Docker v2 provisioning now starts a real managed container
  with canonical instance IDs, management endpoint env, labels, mounts, and a
  one-time `AGENT_BOOTSTRAP_*` envelope instead of failing on the retired
  `AGENT_SECRET` path.
- Updated the v1 container create path to issue bootstrap enrollment material
  when callers have not provided mTLS, UDS, vsock, or bootstrap env.
- Taught the container entrypoint to accept bootstrap enrollment as a secure
  first-start transport so the Rust agent can enroll and reconnect over mTLS.
- Added a VM deploy guard that rejects `agent.env` files without usable secure
  transport material before installing/restarting `agent-client.service`.

### Documentation

- Documented the normalized runtime bootstrap envelope for containers and VMs.

### Verification

- Added a v2 Docker provisioning regression test with a fake Docker binary to
  prove bootstrap env, canonical instance IDs, labels, and mounts are passed to
  `docker run`.
- Re-ran admin v2 tests, container unit tests, bootstrap enrollment tests,
  cloud-init secure-transport tests, loadout generation tests, shell syntax
  checks, and container entrypoint bootstrap acceptance/rejection checks.

## [2026.6.19] — 2026-06-19

> Superseded by `v2026.6.20` for the Docker/VM runtime bootstrap injection
> fix. `v2026.6.19` remains the successful host-runtime bootstrap release.

### Fixed

- Supersedes `v2026.6.18`. The `v2026.6.18` tag includes the host runtime
  bootstrap enrollment fix but its tag CI failed because
  `agentic-host-runtime-daemon` was not updated for the new supervisor config
  field.
- Added `AGENTIC_HOST_BOOTSTRAP_ENROLLMENT_URL` / `--bootstrap-enrollment-url`
  plumbing to the host runtime daemon so daemon-supervised host agents receive
  the same bootstrap enrollment endpoint as embedded host supervisor mode.

### Operator notes

- Use `v2026.6.19` for the host runtime bootstrap enrollment release. Treat
  `v2026.6.17` and `v2026.6.18` as superseded release-attempt tags.
- Verification now includes the full `make test-unit` target, covering
  management, `agent-rs`, and `cli`, after the daemon compile fix.
- The 2026-06-18 live proof remains valid: a host agent registered over mTLS,
  opened a managed `tmux` session, and launched Codex inside that session;
  Codex used AIWG discovery and selected `issue-audit`.
- Claude auth-state propagation remains tracked separately from this host
  runtime registration fix.

## [2026.6.18] — 2026-06-18

> Superseded by `v2026.6.19`. The tag was pushed but failed the tag CI
> workspace test because `agentic-host-runtime-daemon` was missing the new
> bootstrap enrollment config field.

### Fixed

- Supersedes `v2026.6.17` with the same host runtime bootstrap enrollment fix
  plus the required `cargo fmt` correction for the tag lint gate.
- Provisioned host-runtime agents receive one-time bootstrap enrollment
  material and start with `AGENT_TRANSPORT=auto`, allowing them to exchange the
  bootstrap token for mTLS material and register with management over transport
  identity instead of failing on retired plaintext TCP auth.
- Host provisioning operation results expose only non-secret bootstrap evidence
  (`bootstrap_token_issued`, SPIFFE id, and expiry), while plaintext bootstrap
  tokens remain redacted from operation excerpts.

### Documentation

- Documented host-runtime bootstrap enrollment behavior and the
  `AGENTIC_HOST_BOOTSTRAP_ENROLLMENT_URL` override.
- Added the `v2026.6.18` release announcement with live host proof evidence and
  the `v2026.6.17` supersession note.

### Operator notes

- Use `v2026.6.18` for the direct-delivery CalVer release-flow cut that
  restores secure host-agent registration for real agentic-framework session
  proof.
- `v2026.6.17` should be treated as a superseded release-attempt tag. Its tag
  lint gate failed on formatting only; `v2026.6.18` carries the same behavior
  plus the formatting correction.
- The 2026-06-18 live proof registered a host agent over mTLS, opened a
  managed `tmux` session, and launched Codex inside that session; Codex used
  AIWG discovery and selected `issue-audit`.
- Claude launched in the same session but did not inherit usable auth state and
  reported login was required. Treat Claude auth-state injection as a separate
  follow-up from the host runtime registration fix.

## [2026.6.17] — 2026-06-18

> Superseded by `v2026.6.18`. The tag was pushed but failed the tag lint gate
> on `cargo fmt` formatting only.

### Fixed

- Provisioned host-runtime agents now receive one-time bootstrap enrollment
  material and start with `AGENT_TRANSPORT=auto`, allowing them to exchange the
  bootstrap token for mTLS material and register with management over transport
  identity instead of failing on retired plaintext TCP auth.
- Host provisioning operation results now expose only non-secret bootstrap
  evidence (`bootstrap_token_issued`, SPIFFE id, and expiry), while plaintext
  bootstrap tokens remain redacted from operation excerpts.

### Documentation

- Documented host-runtime bootstrap enrollment behavior and the
  `AGENTIC_HOST_BOOTSTRAP_ENROLLMENT_URL` override.
- Added the `v2026.6.17` release announcement with live host proof evidence.

### Operator notes

- Do not use `v2026.6.17`; use `v2026.6.18` instead.
- The 2026-06-18 live proof registered a host agent over mTLS, opened a
  managed `tmux` session, and launched Codex inside that session; Codex used
  AIWG discovery and selected `issue-audit`.
- Claude launched in the same session but did not inherit usable auth state and
  reported login was required. Treat Claude auth-state injection as a separate
  follow-up from the host runtime registration fix.

## [2026.6.16] — 2026-06-17

### Fixed

- Mirrored the aggregate `SHA256SUMS` checksum file to the GitHub release
  alongside per-asset checksum sidecars, so public release consumers can verify
  the complete package set from one canonical checksum manifest.
- Listed registered host-runtime executor contexts in `GET
  /api/v2/admin/instances` even when there is no backing libvirt VM or Docker
  container to enumerate. Host-backed instances now appear with `runtime:
  "host"` and their registered loadout metadata.

### Operator notes

- Use `v2026.6.16` for the direct-delivery CalVer release-flow cut that
  includes the GitHub checksum mirror fix and the host-runtime admin listing
  fix.

## [2026.6.15] — 2026-06-17

### Fixed

- Installed release SBOM/signing tools into a workspace-local `.tools/bin`
  directory instead of `/usr/local/bin`, allowing the tag workflow to run on
  locked-down host runners that do not grant system write access.

### Operator notes

- `v2026.6.14` should be treated as a superseded release-attempt tag. It
  proved the build, E2E, package, crates, container, Gitea release, and GitHub
  mirror lanes, but failed before SBOM generation because the runner could not
  install `syft` into `/usr/local/bin`.
- Use `v2026.6.15` for the direct-delivery CalVer release-flow cut.

## [2026.6.14] — 2026-06-16

### Fixed

- Hardened SBOM/signature asset upload by cleaning syft extraction scratch
  directories before upload and writing upload response bodies under the
  workspace instead of `/tmp`, avoiding late-stage `curl` write failures after
  large image SBOM generation.
- Rotated the GitHub release mirror secret to a token with `repo` scope so the
  required GitHub Releases publication step can create/update the public mirror
  release.

### Operator notes

- `v2026.6.13` should be treated as a superseded release-attempt tag. It
  proved all build, E2E, package, crates, and container lanes, but the final
  SBOM upload and GitHub mirror publication gates failed.
- Use `v2026.6.14` for the direct-delivery CalVer release-flow cut.

## [2026.6.13] — 2026-06-16

### Fixed

- Moved the Apple Silicon host-direct execution check into the already-open
  mutsu build SSH session and changed the post-package smoke to local tarball
  verification. This avoids the extra post-build mutsu SSH setup/scp hop that
  could hang after successful Darwin builds while still proving the binaries
  execute on mutsu and the release archive contains arm64 Mach-O payloads.

### Operator notes

- `v2026.6.12` should be treated as a superseded release-attempt tag. It proved
  the full x86_64 release lanes and produced the Darwin tarball, but the
  additional post-package SSH smoke setup hung after tarball creation.
- Use `v2026.6.13` for the direct-delivery CalVer release-flow cut.

## [2026.6.12] — 2026-06-16

### Fixed

- Hardened the release-blocking mutsu SSH lane so Apple Silicon smoke setup
  and cleanup tolerate slower post-reboot host readiness. The aarch64 release
  workflow now uses longer SSH setup/cleanup timeouts, additional connection
  attempts, and a less aggressive server-alive window while preserving the
  fail-closed release matrix.

### Operator notes

- `v2026.6.11` should be treated as a superseded release-attempt tag. Its
  product builds passed, including the Linux musl ioctl fix, but the
  Apple Silicon host-direct smoke failed while mutsu SSH was being rebooted.
- Use `v2026.6.12` for the direct-delivery CalVer release-flow cut that
  includes the local-first CA backend lifecycle, deterministic CA renewal test,
  target-typed PTY ioctl fix, and hardened mutsu release smoke path.

## [2026.6.11] — 2026-06-16

### Fixed

- Typed the agent controlling-terminal `ioctl(TIOCSCTTY)` request per target so
  Darwin uses the required `c_ulong` request while Linux musl/gnu keep the raw
  Linux ioctl request type.

## [2026.6.10] — 2026-06-16

### Fixed

- Made the local CA agent leaf renewal-window test deterministic by comparing
  the renewed certificate expiry against the original expiry instead of the CI
  runner wall clock.

## [2026.6.9] — 2026-06-16

> **Darwin agent build fix.** This patch preserves the CA backend lifecycle and
> mutsu rust tooling fixes from `v2026.6.8`, and fixes the Apple Silicon
> `agent-client` build by casting the PTY controlling-terminal ioctl request to
> the platform `c_ulong` type.

### Fixed

- **Darwin PTY ioctl portability** (#481): `agent-client` now casts
  `libc::TIOCSCTTY` to `libc::c_ulong` before calling `libc::ioctl`, matching
  the Darwin libc signature while preserving Linux behavior.

### Operator notes

- `v2026.6.8` should be treated as a superseded release-attempt tag. Use
  `v2026.6.9` for the CA backend lifecycle and direct release-flow release.
- The prior `v2026.6.8` tag proved the mutsu Rust tooling path fix by reaching
  Darwin compilation; this patch addresses the next compiler error surfaced by
  that lane.

## [2026.6.8] — 2026-06-16

> **Mutsu rustup path fix.** This patch preserves the CA backend lifecycle and
> direct release-flow content from `v2026.6.7`, and fixes the mutsu SSH release
> lane so it can resolve Rust tooling from the external cargo home, Homebrew,
> or the automation user's cargo home.

### Fixed

- **Mutsu release Rust tool discovery** (#481): the aarch64 SSH build now
  expands `PATH` to include `/Volumes/build/agentic-sandbox/cargo/bin`,
  `$HOME/.cargo/bin`, Homebrew, and the cached stable toolchain path; it also
  prints remote Rust tooling diagnostics and only falls back from `rustup
  target add` when `rustc` can already resolve the requested target.

### Operator notes

- `v2026.6.7` should be treated as a superseded release-attempt tag. Use
  `v2026.6.8` for the CA backend lifecycle and direct release-flow release.
- If mutsu release jobs fail again, inspect the remote tooling diagnostic block
  before changing tag policy; the job now distinguishes missing `rustup` from
  missing target standard libraries.

## [2026.6.7] — 2026-06-16

> **CA backend lifecycle and direct release flow.** This patch completes the
> local-first gRPC mTLS CA backend boundary, documents the operational model,
> and adds the AIWG CalVer release gates used by this direct-delivery project.

### Added

- **gRPC CA backend lifecycle** (#492/#493): management now selects a
  configurable CA backend for agent mTLS, defaults workstation deployments to
  the embedded local CA, exposes a fail-closed remote backend boundary, and
  renews agent/server leaves according to the configured validity windows.
- **Direct-delivery CalVer release flow config**: `.aiwg/release.config`
  records the project release gates, code-to-docs sync step, Gitea CI checks,
  annotated CalVer tag publishing, and release asset verification path.

### Documentation

- **Code-to-docs sync for secure transport**: the README roadmap now marks the
  authenticated transport slice complete and points operators to the CA backend
  operations guide.

### Operator notes

- Workstation/local deployments use the embedded CA by default. Set
  `AGENTIC_GRPC_CA_BACKEND=remote` only when testing fail-closed remote CA
  behavior; use `remote-mock` for provider-boundary integration tests.

## [2026.6.6] - 2026-06-15

> **Mutsu SSH timeout fix.** This patch preserves the packaged release
> pipeline content from `v2026.6.5` and bounds every mutsu SSH/SCP operation
> so tag CI cannot block indefinitely when the Apple Silicon host or SSH auth
> path is unavailable.

### Fixed

- **Mutsu release job bounded timeouts** (#481): release CI now uses
  `BatchMode`, explicit connect and keepalive options, and shell `timeout`
  wrappers around mutsu keyscan, build, artifact copy, smoke-test, and cleanup
  operations.

### Operator notes

- `v2026.6.5` should be treated as a superseded release-attempt tag. Use
  `v2026.6.6` for the packaged release pipeline assets.

## [2026.6.5] - 2026-06-15

> **GHCR latest fix.** This patch preserves the packaged release pipeline
> content from `v2026.6.4` and stamps public GHCR `latest` tags from the
> release-tagged internal image instead of requiring every internal image to
> already have a `latest` tag.

### Fixed

- **GHCR latest mirroring** (#478): release CI now pulls each internal image by
  the immutable release tag once, then pushes both the versioned GHCR tag and
  GHCR `latest` from that same source image.

### Operator notes

- `v2026.6.4` should be treated as a superseded release-attempt tag. Use
  `v2026.6.5` for the packaged release pipeline assets.

## [2026.6.4] - 2026-06-15

> **GHCR namespace fix.** This patch preserves the packaged release pipeline
> content from `v2026.6.3` and fixes GHCR publication from Gitea by using the
> GitHub package namespace instead of the Gitea repository owner.

### Fixed

- **GHCR owner mapping** (#478): release CI now publishes, smoke-tests, SBOMs,
  and signs public images under `ghcr.io/${GHCR_OWNER:-jmagly}` instead of
  deriving the namespace from Gitea's `github.repository_owner`.

### Operator notes

- `v2026.6.3` should be treated as a superseded release-attempt tag. Use
  `v2026.6.4` for the packaged release pipeline assets.

## [2026.6.3] - 2026-06-15

> **Release rerun cut.** This patch preserves the packaged release pipeline
> content from `v2026.6.2` under a fresh signed tag because the original Gitea
> tag workflow started before release-blocking secrets were available and this
> Gitea version does not expose cancel/rerun/dispatch Actions APIs.

### Changed

- **Release identity refresh** (#462/#478/#479/#480/#481): bump package and
  binary versions to `2026.6.3` so tag CI can publish the packaged release
  matrix with the now-present `MUTSU_SSH_KEY` and `GHCR_TOKEN` secrets.

### Operator notes

- `v2026.6.2` should be treated as a superseded release-attempt tag. Use
  `v2026.6.3` for the packaged release pipeline assets.

## [2026.6.2] — 2026-06-14

> **The packaged release pipeline cut.** This patch promotes release
> publication from raw tarballs plus internal images to a verified package
> matrix: native Linux packages, a checksum-verifying installer, public GHCR
> runtime images, and fail-closed Apple Silicon host-direct publication.

### Added

- **Native Linux release packages** (#479): tag CI now builds `.deb` and
  `.rpm` assets for x86_64 Linux, packages the management server, host runtime
  daemon, event bridge, agent client, `sandboxctl`, and the documented
  `agentic-sandbox` CLI alias under `/usr/bin`.
- **Package-owned service assets** (#479): release packages include
  systemd units and env templates under stable package-managed paths.
- **HotM-style Linux installer** (#480): `agentic-sandbox-install.sh`
  resolves latest or pinned releases, downloads native packages, verifies
  checksums before install, supports local package validation, and smoke-checks
  installed commands.
- **Public GHCR runtime package matrix** (#478): release CI mirrors
  management, agent-client, agent, claude, codex, opencode, and
  automation-control images to `ghcr.io/<owner>/agentic-sandbox-*` with
  version and stable `latest` tags.
- **Release matrix verification tests** (#478/#480): package smoke tests cover
  clean Debian/RPM-family install-uninstall paths, installer parser/checksum
  behavior, and GHCR workflow/doc matrix consistency.

### Changed

- **Release publication fails closed** (#478/#481): tag releases now require
  `GHCR_TOKEN` for public GHCR packages and `MUTSU_SSH_KEY` for Apple Silicon
  host-direct artifacts instead of silently skipping supported release
  surfaces.
- **Apple Silicon scope is explicit** (#481): current macOS support is Docker
  plus host-direct `sandboxctl`/`agent-client` tarballs built on mutsu over SSH.
  `.dmg`, `agentic-mgmt`, `vm-event-bridge`, and Apple VM/provider packaging
  remain deferred to the future macOS provider/app story.
- **Windows is explicitly deferred** (#482): the current release matrix does
  not publish Windows installers. The likely first Windows package is a future
  `sandboxctl.exe` operator-client build after a pinned Windows builder and
  smoke-test lane exist.

### Fixed

- **Installer latest-release resolution** (#480): the Python tag parser now
  matches valid `vYYYY.M.P` tags and status logging no longer contaminates the
  command substitution used to resolve `latest`.
- **Package metadata validation** (#479): CI now validates the Debian
  `agentic-sandbox -> sandboxctl` symlink using the format emitted by
  `dpkg-deb --contents`.

### Documentation

- **Release runbook package flow** (#462/#478/#479/#480/#481): the runbook now
  documents native package assets, GHCR pulls, a compose-style management
  image example, installer usage, macOS host-direct tarball checks, and the
  Windows deferral.
- **Release audit matrix** (#462): the release-pipeline audit records the
  supported package surfaces, required secrets, AppImage deferral rationale,
  and tag-context proof still required before closing release-completion
  issues.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.6.2`.**
- **Required release secrets:** production tag releases now require
  `GHCR_TOKEN` and `MUTSU_SSH_KEY`; missing values fail release publication
  with actionable errors.
- **Linux install path:** use the published `agentic-sandbox-install.sh`
  script or install the `.deb`/`.rpm` package directly.
- **Release source of truth:** tag CI remains authoritative. Package issues
  close only after tag CI publishes the new assets and the post-tag verifier
  confirms GHCR, package, installer, and mutsu evidence.

### Issues closed

- #482 — Windows installer parity decision recorded as deferred for the
  current release matrix.

## [2026.6.1] — 2026-06-14

> **The full-autonomy substrate release.** This patch completes the
> post-2026.6.0 transport-security hardening slices that remove legacy shared
> secret and TOFU defaults, and adds the executor-substrate axis AIWG needs:
> bare-host execution plus direct and managed session-host control over
> `pty-ws/v1`.

### Added

- **Bare-host execution target for AIWG's base level** (#460): host runtime
  metadata and isolation-tier reporting, a local host supervisor boundary,
  host lifecycle routing through admin v2, and an explicit
  `AGENTIC_HOST_RUNTIME_MODE=daemon` path for process supervision without a
  Docker or VM wrapper.
- **First-party host runtime daemon** (#460): `agentic-host-runtime-daemon`
  serves the host supervisor over a fail-closed Unix-domain socket protocol,
  with documented systemd user-unit wiring for operators who need durable
  bare-host agents.
- **Session-host backend contract and selection** (#461): `pty-ws/v1`
  advertises session host capabilities and accepts operator-selected
  backend/class pairs through the v2 executor contract.
- **Direct and managed session control** (#461): native/direct PTY sessions
  plus managed `tmux`, `screen`, and `zellij` session wrappers are wired
  through `AgentPtyBridge` with conformance coverage.
- **Fast host-target PTY conformance proof** (#460): multiple
  `RuntimeKind::Host` instances on one host attach over `pty-ws/v1`, keep
  output isolated, forward controller stdin through `PtyBridge`, and reattach
  via replay keyframe.
- **Secure transport provisioning path** (#409/#410/#412): opt-in gRPC UDS,
  vsock, and mTLS transport primitives, peer identity resolution, auth-context
  wiring, embedded local CA provisioning, bootstrap token and CSR enrollment
  APIs, and secure agent image/loadout provisioning.

### Changed

- **Secure transport becomes the default for VM provisioning** (#412): secure
  loadouts omit legacy agent shared secrets and provision the local CA / mTLS
  bootstrap path by default.
- **Legacy shared secrets are compatibility-only** (#412): legacy agent
  secrets are no longer emitted for secure transports and TOFU is disabled by
  default.
- **Release/docs delivery surface refreshed**: the README now reflects the
  Rust-native testing story and current roadmap, the docsite publishing path
  uses pagenary, and internal Git host references were removed from public
  docs.

### Fixed

- **Stale E2E IP allocations are reaped** before integration runs, reducing
  recurrence risk after cancelled or interrupted VM-backed tests.
- **Legacy shared-secret retirement is explicit** in docs and bootstrap
  behavior, so operators can distinguish compatibility paths from the secure
  default path.

### Documentation

- **Transport phase acceptance docs** record the completed UDS/vsock/mTLS
  local-first transport slices and the legacy-secret retirement plan.
- **Host runtime and daemon docs** describe local process supervision,
  daemon-mode fail-closed behavior, socket permissions, and operator wiring.
- **Conformance protocol docs** now name the host-target `pty-ws/v1` proof as
  the fast T0/T4 guard before future live daemon T4 runs.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.6.1`.**
- **Full autonomy substrate axis:** AIWG can now select host, Docker, or VM
  per instance; host is explicitly the least-isolated tier and should be shown
  as full host access in operator UX.
- **Host daemon deployment remains operator-owned:** the release ships the
  daemon binary and example systemd unit but does not install or start a
  persistent host service by default.
- **Transport-security Phase 4 remains gated:** external CA integration is
  still waiting on operator OpenBao genesis; OpenBao tokens/unseal material
  must stay out of repos and commlog.
- **Branch and tag CI remain release source of truth:** VM-backed E2E and
  conformance passed on the #460/#461 delivery slices and will run again in
  tag context for this release.

### Issues closed

- #409 — local-first gRPC UDS/vsock/mTLS transport groundwork and identity
  plumbing.
- #410 — embedded local CA and bootstrap enrollment provisioning path.
- #412 — legacy shared-secret / TOFU removal for secure transports.
- #460 — local user-host execution target with daemon supervision and host
  PTY conformance proof.
- #461 — direct and managed session control via native, tmux, screen, and
  zellij backends.

## [2026.6.0] — 2026-06-11

> **The Rust E2E migration release.** The legacy pytest E2E harness is fully
> retired: the VM-backed gate now runs Rust-native suites end to end, joined
> by two new conformance tiers (live-agent and restart-durability). The CI
> lane that produces release evidence self-heals its recurring titan Docker
> wedge and can no longer hang to the platform limit. The repository adopts
> AGPL-3.0-only licensing.

### Added

- **Rust E2E suites replacing the legacy pytest harness** (#302): server
  health and agent registration, command dispatch, concurrency, agent info,
  and a WebSocket connect/idle parity slice
  (`rust_e2e_websocket_connects_and_stays_open`).
- **VM-backed Rust resource-limit E2E coverage** (#302): memory pressure
  (swap-aware), file-descriptor limits, IO throttling, agentshare write and
  quota enforcement, nonzero-exit propagation, and PID stress driven through
  agent dispatch.
- **Live-agent conformance tier (T3)** (#281): terminal
  `completed`/`failed`/`canceled`/`rejected` shapes, HITL prompt and response
  paths including invalid-response `422`, bounded `adapter-command/v1`, and a
  synthetic-fixtures-only report script — no real credentials or environment
  probing.
- **Restart-durability conformance tier** (#283), with a deterministic PTY
  bridge in conformance mode (#282).
- **Mission durability**: poisoned missions are quarantined on resync, and
  mission correlation ids propagate through dispatch logs.
- **Observability** (#188): output-aggregator backpressure metrics, libvirt
  RPC timings traced by operation, formal PTY session-resize decision
  tracing, and session replay summary logging.
- **VM event bridge ported to Rust**, removing a Python runtime dependency
  from the event path.
- **Browser-QA hardening**: carbonyl sessions persist across QA runs (#318)
  and wait-ready diagnostics are hardened (#381). A TUI redraw stress
  harness joins the UI test surface (#373).
- **Transport-security spikes**: rustls hot-reload and native vsock tonic
  prototypes land as groundwork for the accepted transport plan — no
  transport behavior change ships in this release.

### Changed

- **Adopt AGPL-3.0-only licensing** across repository documentation and Rust
  crate metadata (#372).
- **Legacy pytest E2E gate retired** (#302): the `tests/e2e` pytest harness
  is removed and the CI E2E gate is Rust-only; tracked bytecode and the
  obsolete pytest CI setup are cleaned up.
- CLI attach migrates to the `pty-ws/v1` protocol (#254).
- E2E gates every push to `main` and PR branches, not just release tags.
- Docsite workflows updated for pagenary publishing (#376).

### Fixed

- **titan Docker lane self-heals and fails fast** (#335): the docker
  preflight probes daemon readiness with a hard timeout, restarts a wedged
  `dockerd` in-job, and fails with a clear error instead of hanging; the
  VM-backed E2E step is bounded by an effective in-step shell timeout so a
  hung lane can no longer run to the platform limit (#363).
- **Stale `agentic-e2e-*` VMs are reaped before integration runs**, fixing
  the orphan-VM accumulation that exhausted runner memory.
- E2E runner substrate preflight logging (#367) and a bounded release E2E
  runner lane (#363) make lane health visible per run.
- systemd notify watchdog enabled for the management service (#275).
- Management API libvirt VM operations are bounded, preventing unbounded
  blocking calls.
- VM resource-limit E2E hardened: swap-aware memory stress target and a
  poison-recovering test lock eliminate collateral test failures.
- TUI redraw screen state stabilized (#353); hot replay search output
  bounded (#351).
- Release job gating hardened (#341); releases fail fast when the GitHub
  mirror token is missing.
- Rust E2E reads root-owned VM info with a sudo fallback (#402).

### Documentation

- **Agent transport security plan** added with its ADR gate accepted —
  design groundwork for UDS/vsock/mTLS transports; implementation
  intentionally deferred.
- **Strict docs link validation** wired into the CI lint job with stale
  link and anchor repairs across the docs tree (#196).
- Reliability docs linked from the docs index; Codex image pin repaired.
- Docker runner exec recovery procedure documented.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.6.0`.**
- **License change**: the repository is now AGPL-3.0-only (#372).
  Downstream consumers should review the license obligations.
- The legacy pytest harness (`tests/e2e/`) no longer exists; operator
  tooling that referenced it should target the Rust suites under
  `management/tests/`.
- VM-backed E2E and release-critical tag jobs continue to require the
  `titan` runner lane (unchanged since v2026.5.17); runner substrate drift
  tracking remains open in #363/#367.

## [2026.5.17] — 2026-05-24

> **Release-critical CI runner hardening patch.** This release supersedes v2026.5.16, whose signed tag was pushed but whose tag CI failed before repository checkout because the `teroknor` Docker runner could not pull `docker.gitea.com/runner-images:ubuntu-latest` for the pre-release validation job. It keeps the v2026.5.16 documentation sync and moves release-critical tag jobs onto the already-proven `titan` release runner.

### Fixed

- **Release-critical jobs no longer depend on the teroknor Docker runner image pull** (#369): `prerelease-gate`, `release-binaries-mutsu`, `release-attach`, and `github-release-sync` now run on `titan`, matching the build/test/docker/E2E release lane that was already active in the same tag pipeline. The non-blocking security scan remains on `teroknor` with `continue-on-error: true`.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.17.md` documents the blocked v2026.5.16 tag, the release workflow hardening, and the superseding release path.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.17`**.
- **v2026.5.16 is superseded**: the tag was signed and pushed to both Gitea and GitHub, but tag CI run 639 failed before checkout in `Pre-release Validation` due to an upstream runner image pull HTTP 500. Do not treat v2026.5.16 as the clean published release.
- **No runtime behavior change beyond v2026.5.16**: this patch carries the documentation sync from v2026.5.16 plus release workflow hardening so publication can run from a fixed tagged commit.
- **Release publication gate remains intact**: Gitea release attachment, crates.io publication, GitHub release mirroring, and public registry mirroring still wait for release-blocking tag CI and E2E.

### Issues closed

- #369 - release-critical tag jobs depend on teroknor docker runner image pull.
- #370 - prepare v2026.5.17 to supersede blocked v2026.5.16 tag.


## [2026.5.16] — 2026-05-24

> **Operator documentation synchronization release.** This patch release carries the AIWG doc-sync/code-to-docs pass after v2026.5.15. It keeps the v2026.5.15 substrate baseline intact and makes the README, quickstart, how-to, loadout, container-runtime, QEMU, operations, deployment, and troubleshooting guides match the current task API, image catalog, loadout registry, and runtime notes.

### Documentation

- **Task submission examples now match the live API** (`0392e13`): README, Getting Started, Deployment, and Operations examples now use task manifests via `sandboxctl task submit --file` or REST payloads shaped as `{ "manifest": {...} }`, matching `POST /api/v1/tasks` in `management/src/http/tasks.rs` and the CLI behavior in `cli/src/cmd/task.rs`.
- **Loadout profile docs match the checked-in registry** (`0392e13`): `docs/LOADOUTS.md` now uses the current framework/provider names from `images/qemu/loadouts/registry.json`, including `all`, `aiwg-dev`, `sdlc`, `ops`, `forensics`, and `research`.
- **Container image docs match the curated catalog** (`0392e13`): `docs/container-runtime.md` and `docs/ECOSYSTEM.md` now describe the current provider image set, with `agentic/claude:latest`, `agentic/codex:latest`, `agentic/opencode:latest`, and `agentic/automation-control:latest`; `agentic/agent:dev` remains documented as the shared dev base layer.
- **QEMU runtime guide refreshed** (`0392e13`): `images/qemu/README.md` now reflects the current Go install behavior and describes `aiwg` as AIWG CLI/framework tooling.
- **AIWG doc-sync evidence recorded** (`0392e13`): the run is captured in `.aiwg/.last-doc-sync`, `.aiwg/reports/doc-sync-last-run.json`, and `.aiwg/reports/doc-sync-20260524T190301Z.md` with validation notes.

### Tests

- **Documentation sync validation**: stale-claim grep passed for old task examples, old model names, old image tags, stale framework names/counts, `Go 1.22`, and `AI Writing Guide`.
- **Markdown link validation**: targeted relative-link check passed across 11 operator-facing docs.
- **Formatting and script checks**: `git diff --check`, `make lint`, and `bash -n images/qemu/loadouts/resolve-manifest.sh images/qemu/loadouts/generate-from-manifest.sh images/qemu/provision-vm.sh` passed.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.16`**.
- **No substrate behavior change**: this is a documentation and release-manifest patch on top of the v2026.5.15 runtime baseline.
- **Branch CI proof before release prep**: push runs 634, 635, and 636 passed on signed commit `0392e13`, covering the main CI workflow, conformance harness, and supply-chain pin policy.
- **Tag CI remains the release source of truth**: release-blocking E2E, artifact publication, crate publication, release mirroring, and container mirroring should run only after the `v2026.5.16` tag is pushed.

### Issues closed

- #368 — prepare v2026.5.16 documentation sync release.


## [2026.5.15] — 2026-05-24

> **Base-image verifier diagnostics patch.** This release supersedes v2026.5.14, whose tag workflow correctly blocked publication but failed E2E before VM boot because the CI job observed an implausibly small base-image file length for a valid compressed qcow2 on a `titan`-labeled runner. It keeps the release gate enforcement from v2026.5.14 and makes the backing-file verifier diagnose runner path differences while accepting valid qcow2 metadata through to manifest and sha verification.

### Fixed

- **Base-image verifier path-view diagnostics** (#366): `images/qemu/lib/verify.sh` now emits `stat`, `ls`, `qemu-img info`, mount, and manifest context when qcow2 validation fails, so CI failures identify whether the runner saw a compressed/sparse file, stale file, partial copy, wrong mount, format mismatch, manifest mismatch, or sha mismatch.
- **Compressed/sparse qcow2 sanity handling** (#366): raw file length below the default 1 GiB threshold no longer fails by itself when `qemu-img` reports qcow2 format and a sane virtual size; the verifier continues to manifest size and sha256 checks, which remain fail-closed for tampering or stale path views.

### Tests

- **Verifier regression coverage**: `images/qemu/tests/test-verify.sh` covers small-file qcow2 metadata acceptance, manifest size mismatch failure, and diagnostics emitted for undersized files without qemu metadata.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.15.md` documents the v2026.5.14 tag failure, the verifier patch, and the release-gate behavior.
- **Base-image rotation guide**: `images/qemu/docs/base-image-rotation.md` now describes virtual-size-aware sanity checks and failure diagnostics.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.15`**.
- **v2026.5.14 is superseded**: tag run 627 correctly skipped release publication after E2E failed at base-image verification, but it did not produce a clean release. Use v2026.5.15 or newer as the release-gated automation-control/TUI baseline.
- **Release publication gate remains intact**: Gitea release attachment, crates.io publication, GitHub release mirroring, and public registry mirroring still wait for tag E2E.

### Issues closed

- **#366** — base image sanity check rejects valid compressed qcow2 file length.

## [2026.5.14] — 2026-05-24

> **Release gate enforcement and manual E2E runner patch.** This release supersedes v2026.5.13, whose tag workflow failed release-blocking E2E while still allowing publication jobs to run. It keeps the VM substrate fixes from v2026.5.13 and adds the missing publication dependency on tag E2E, plus a self-contained Python venv path for one-off titan substrate validation.

### Fixed

- **Release publication E2E gate** (#364): tag publication jobs now depend on the tag-only E2E job, so failed release-blocking VM substrate validation prevents Gitea release attachment, crates.io publication, GitHub release mirroring, and public registry mirroring.
- **Manual E2E Python environment** (#365): `scripts/run-e2e-tests.sh` now creates and uses a local virtual environment when Python is not already running inside one, so PEP 668 hosts can run substrate validation without caller-managed pip setup.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.14.md` documents the v2026.5.13 tag failure, the release-publication gate fix, manual titan VM E2E proof, and the remaining runner-isolation risk.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.14`**.
- **v2026.5.13 is superseded**: the release artifacts were created, but tag CI run 614 failed release-blocking E2E while publication jobs still ran. Use v2026.5.14 or newer as the clean automation-control/TUI release-gate baseline.
- **Manual titan VM E2E proof**: a fresh temp clone at `4a413a4` passed `E2E_CLEANUP_VM=1 AGENTIC_VM_SSH_WAIT_SECONDS=900 E2E_VM_READY_TIMEOUT=900 ./scripts/run-e2e-tests.sh` with `25 passed, 4 skipped in 96.02s`; the VM reached SSH in about 9 seconds and was destroyed after the run.
- **Branch CI proof before release prep**: runs 622, 623, and 624 passed on signed commit `4a413a4`, covering lint, unit tests, build, Docker publish, security scan, supply-chain pin lint, and conformance. Branch E2E is intentionally skipped; tag CI remains the release source of truth for release-blocking E2E.
- **Runner isolation risk tracked separately**: #363 documents titan host-runner contention from unrelated heavy builds. The substrate is manually green; deterministic release validation still depends on keeping the titan E2E lane quiet or isolated.

### Issues closed

- **#364** — tag publication jobs must depend on release-blocking E2E.
- **#365** — manual E2E runner should create its own venv on PEP 668 hosts.

## [2026.5.13] — 2026-05-24

> **VM substrate release-gate repair.** This release supersedes v2026.5.12, whose tag workflow created artifacts but still failed release-blocking E2E after exposing two deeper titan VM substrate defects: the runner had blessed a truncated Ubuntu agent base image, and provisioned VMs booted BIOS-style while the project image builder creates UEFI images.

### Fixed

- **Base-image sanity gate** (#362): `images/qemu/lib/verify.sh` now rejects implausibly small qcow2 base images before recording or trusting a manifest entry, records `size_bytes`, `virtual_size_bytes`, and `format`, and verifies manifest size metadata when present.
- **Provisioning timeout propagation** (#362): `scripts/reprovision-vm.sh` now preserves `AGENTIC_VM_SSH_WAIT_SECONDS` and `SSH_WAIT_SECONDS` through the second `sudo` boundary before invoking `provision-vm.sh`.
- **UEFI provisioning for project-built images** (#362): libvirt VM definitions now default to OVMF/UEFI boot, matching `build-base-image.sh --boot uefi`; `AGENTIC_VM_FIRMWARE=bios` remains available for BIOS-built images.
- **Titan runner base image repaired** (#362): the bad 193 KiB `/mnt/ops/base-images/ubuntu-server-24.04-agent.qcow2` was backed up and rebuilt from the pinned Ubuntu 24.04.3 live-server ISO. The new manifest records sha256 `0fc2b1a3b443c143a03454a324a5c2223e6e39ae7dfed9642bf1775d34c39c93` with `size_bytes: 4521590784`.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.13.md` documents the base-image guard, UEFI boot fix, titan host repair, manual VM E2E proof, and superseded v2026.5.12 tag.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.13`**.
- **v2026.5.12 is superseded**: the release artifacts were created, but tag CI run 602 failed release-blocking E2E because titan's base image was a 193 KiB qcow2 placeholder and provisioned VMs did not boot UEFI images with OVMF.
- **Manual titan VM E2E proof**: after rebuilding the base image and applying the UEFI provisioner fix, `make test-e2e` passed on titan with `25 passed, 4 skipped in 96.63s`; the VM reached SSH in about 13 seconds and was destroyed after the run.
- **Branch CI proof before release prep**: runs 610 and 611 passed on signed commit `5b8ec7d`, covering lint, unit tests, build, Docker publish, security scan, supply-chain pin lint, and conformance. Branch E2E is intentionally skipped; tag CI remains the release source of truth for release-blocking E2E.
- **Runner contention risk tracked separately**: #363 documents titan host-runner contention from unrelated heavy builds; cut the tag when the titan lane is quiet or isolated so tag E2E measures the sandbox substrate rather than shared-runner saturation.

### Issues closed

- **#362** — titan runner blessed a truncated VM base image and provisioned VMs did not boot UEFI images.

## [2026.5.12] — 2026-05-23

> **Release-gate heartbeat patch.** This release supersedes v2026.5.11, whose tag workflow created artifacts but still failed E2E while waiting for first-boot VM SSH. The previous fixes correctly preserved the 900s wait configuration, but the provisioning loop could stay quiet long enough for the runner to treat the job as stalled. This patch emits bounded SSH wait progress so tag E2E can either complete or fail with script-owned diagnostics.

### Fixed

- **Provision SSH wait heartbeat** (#360): `images/qemu/provision-vm.sh` now logs SSH wait progress every 30 seconds by default while waiting for first-boot SSH, with `AGENTIC_VM_SSH_PROGRESS_SECONDS` available for tuning. This prevents silent long waits from being mistaken for dead CI jobs and preserves the actionable VM diagnostics added in the previous release-gate patches.
- **Release runner label availability** (#361): the active Gitea runner was re-declared with the `titan` host label so existing Rust/build/E2E/conformance jobs can be scheduled without changing workflow semantics.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.12.md` documents the heartbeat patch, branch CI proof, and the superseded v2026.5.11 tag.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.12`**.
- **v2026.5.11 is superseded**: the release artifacts were created, but tag CI run 595 failed the release-blocking E2E job after the 900s wait env was preserved because the SSH wait loop remained quiet long enough to be treated as stalled. Use v2026.5.12 or newer as the clean automation-control/TUI release.
- **Branch CI proof before release prep**: runs 597, 598, and 599 passed on `465de0a`, covering lint, unit tests, build, Docker publish, security scan, supply-chain pin lint, and conformance. Tag CI remains the release source of truth for release-blocking E2E.

### Issues closed

- **#360** — provision SSH wait emits progress during long first-boot waits.
- **#361** — active release runner advertises the `titan` label required by existing CI workflows.

## [2026.5.11] — 2026-05-23

> **Sudo-preserved E2E wait patch.** This release supersedes v2026.5.10, whose tag workflow created artifacts but still failed E2E because `sudo` dropped the workflow-provided SSH wait env before provisioning. It preserves the wait configuration through `sudo env` and improves early-failure diagnostics for root-owned VM state.

### Fixed

- **Sudo-preserved provisioning wait env** (#359): `scripts/run-e2e-tests.sh` now passes `AGENTIC_VM_SSH_WAIT_SECONDS` and `SSH_WAIT_SECONDS` through `sudo env` when invoking `reprovision-vm.sh`, so tag E2E honors the release workflow's 900s first-boot SSH window.
- **Root-owned early-failure diagnostics** (#359): E2E diagnostics now print effective wait env, check VM directories and SSH keys with `sudo test`, and include a bounded VM directory listing so early provision failures no longer misreport generated root-owned files as missing.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.11.md` documents the sudo-preserved E2E wait patch and superseded v2026.5.10 tag.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.11`**.
- **v2026.5.10 is superseded**: the release artifacts were created, but tag CI run 588 failed the release-blocking E2E job while waiting for SSH because the 900s wait env was not preserved through `sudo`. Use v2026.5.12 or newer as the clean automation-control/TUI release.
- **v2026.5.11 is superseded by v2026.5.12**: tag CI run 595 created artifacts but failed release-blocking E2E after the SSH wait configuration was preserved; v2026.5.12 adds bounded SSH wait progress so the runner does not treat the long wait as stalled.
- **Local VM E2E verification passed** on this host with `25 passed, 4 skipped` using `E2E_CLEANUP_VM=1 AGENTIC_VM_SSH_WAIT_SECONDS=300 E2E_VM_READY_TIMEOUT=360 ./scripts/run-e2e-tests.sh`.

### Issues closed

- **#359** — sudo provisioning preserves SSH wait env and diagnostics inspect root-owned state.

## [2026.5.10] — 2026-05-23

> **Release E2E diagnostics and cleanup patch.** This release supersedes v2026.5.9, whose tag workflow created artifacts but still failed the release-blocking E2E job while waiting for first-boot VM SSH. It keeps the v2026.5.9 readiness fixes and adds actionable VM diagnostics, earlier auto-VM cleanup registration, and a longer tag E2E SSH window.

### Fixed

- **Provisioning-failure diagnostics and cleanup** (#358): `scripts/run-e2e-tests.sh` now marks auto-created E2E VMs for cleanup before invoking reprovisioning, and failed provision/readiness paths emit bounded `virsh`, VM metadata, SSH-key presence, DHCP, and QEMU-log diagnostics.
- **Tag E2E first-boot SSH window** (#356): the release E2E workflow now sets `AGENTIC_VM_SSH_WAIT_SECONDS=900` and `E2E_VM_READY_TIMEOUT=900`, keeping local defaults shorter while allowing slower CI first boots to complete or produce diagnostics.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.10.md` documents the E2E diagnostics/cleanup patch and superseded v2026.5.9 tag.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.10`**.
- **v2026.5.9 is superseded**: the release artifacts were created, but tag CI run 583 failed the release-blocking E2E job while waiting for SSH on the first-boot VM. Use v2026.5.11 or newer as the clean automation-control/TUI release.
- **v2026.5.10 is superseded by v2026.5.11**: tag CI run 588 created artifacts but failed release-blocking E2E because the 900s first-boot SSH wait env was dropped across `sudo`.
- **Local VM E2E verification passed** on this host with `25 passed, 4 skipped` using `E2E_CLEANUP_VM=1 AGENTIC_VM_SSH_WAIT_SECONDS=300 E2E_VM_READY_TIMEOUT=360 ./scripts/run-e2e-tests.sh`.

### Issues closed

- **#358** — provisioning failures emit VM diagnostics and clean up auto VMs.

## [2026.5.9] — 2026-05-22

> **Clean substrate release-gate patch.** This release supersedes v2026.5.8, whose tag workflow created artifacts but failed E2E after exposing two additional VM substrate assumptions: first-boot SSH needed a longer bounded wait, and basic-profile VMs should not wait for an agentic-dev setup marker. It also gates disk-quota enforcement tests on real host project-quota support.

### Fixed

- **Configurable provision-time SSH wait** (#356): `provision-vm.sh --wait` and `--wait-ready` now default to a 300s SSH wait and honor `AGENTIC_VM_SSH_WAIT_SECONDS` or `SSH_WAIT_SECONDS`, preventing tag E2E from failing at the previous hardcoded 120s first-boot ceiling.
- **Basic profile setup readiness** (#356): `--wait-ready` now waits for `/opt/agentic-setup/check-ready.sh` only when the VM actually exposes that script, so basic SSH-only VMs no longer block on an agentic-dev readiness marker they never create.
- **Disk quota E2E capability gate** (#357): `test_disk_quota_blocks_excess_write` now skips on hosts without XFS project quotas instead of writing tens of GiB to an unbounded ext4-backed agentshare mount until timeout.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.9.md` documents the clean release-gate patch and the superseded v2026.5.8 tag.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.9`**.
- **v2026.5.8 is superseded**: the release artifacts were created, but tag CI run 578 failed the release-blocking E2E job while waiting for SSH on the first-boot VM. Use v2026.5.11 or newer as the clean automation-control/TUI release.
- **v2026.5.9 is superseded by v2026.5.10**: tag CI run 583 created artifacts but failed release-blocking E2E while waiting for first-boot VM SSH readiness.
- **Local VM E2E verification passed** on this host with `25 passed, 4 skipped` using `E2E_CLEANUP_VM=1 AGENTIC_VM_SSH_WAIT_SECONDS=300 E2E_VM_READY_TIMEOUT=360 ./scripts/run-e2e-tests.sh`.

### Issues closed

- **#356** — tag VM readiness gate timed out before first-boot SSH was available.
- **#357** — disk quota E2E skips when host project quota support is unavailable.

## [2026.5.8] — 2026-05-22

> **Release-gate and Codex automation patch.** This release supersedes v2026.5.7's failed tag E2E gate by making the tag workflow initialize agentshare before VM provisioning. It also promotes the low-churn Codex TUI profile discovered during live validation into a first-class automation-control helper.

### Added

- **Low-churn Codex automation launcher** (`c681e85`, #353): adds `agentic-codex-automation` for automation-control Docker images and VM/QEMU loadouts. The wrapper runs Codex with `TERM=xterm`, `NO_COLOR=1`, and `--no-alt-screen` so browser observers and external orchestrators have a stable default provider-TUI launch path.

### Fixed

- **Tag E2E agentshare bootstrap** (#355): the release E2E workflow now initializes agentshare with `images/qemu/setup-agentshare.sh` when `/srv/agentshare/global` or `global-ro` is missing, instead of failing before VM tests can start. Existing initialized runners are skipped idempotently.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.8.md` documents the release-gate repair, the Codex automation launcher, and the superseded v2026.5.7 tag.
- **Automation-control docs**: `docs/container-runtime.md` and `docs/LOADOUTS.md` describe `agentic-codex-automation`.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.8`**.
- **v2026.5.7 is superseded**: the release page/artifacts were created, but tag CI run 565 failed E2E because agentshare was not initialized on titan. Use v2026.5.11 or newer as the clean automation-control/TUI release.
- **v2026.5.8 is superseded by v2026.5.9**: tag CI run 578 created artifacts but failed release-blocking E2E while waiting for first-boot VM SSH readiness.
- **Preferred Codex launch command**: `agentic-codex-automation`. Set `AGENTIC_CODEX_WORKDIR` when a non-default start directory is needed.
- **Known follow-ups remain open**: #351 tracks `tui search` semantics for hot snapshot text vs durable transcript spill; #353 continues to track browser reconnect/redraw stress coverage and Codex-specific Controller Enter semantics.

### Issues closed

- **#355** — tag E2E agentshare initialization.

## [2026.5.7] — 2026-05-22

> **Automation-control and TUI orchestration release.** This release turns the v2/A2A substrate into a practical launchpad for supervised provider TUI sessions: orchestrators can create named PTY sessions, observe them without write authority, search durable transcript history, launch the automation-control loadout, and start Codex-style provider TUIs directly in tmux. It also hardens VM readiness, A2A artifacts, replay bounds, event memory, and role-gated controller writes.

### Added

- **Orchestrator TUI driver commands** (`36cfa40`, #345): `sandboxctl tui snapshot`, `observe`, `send`, and `search` give external orchestrators a CLI for reading and driving PTY sessions. Observer is the default role; Controller writes require explicit `--yes-controller`.
- **Automation-control blueprint** (`8a045af`, #347): adds a Docker image, VM/QEMU loadout profile, credential-free `agentic-provider-inventory` helper, image catalog entry, docs, and CI smoke coverage for provider-TUI automation/control experiments.
- **Hot event memory metrics** (`29963b2`, #334): exposes Prometheus metrics for the bounded `/api/v1/events` hot window, including resident counts, source counts, capacity, accepted totals, and evictions.
- **Durable mission/event archive** (`b9a27f3`, #336): evicted non-PTY mission/task events now spill to `events.jsonl` and can be explicitly queried with `include_archived=true`.

### Changed

- **Formal PTY replay is bounded to the hot window** (`aa72e71`, #332): new sessions default to a three-screen hot replay window so attach/reconnect stays bounded for long-lived TUI agents.
- **PTY session creation returns orchestrator metadata** (`e0dbeea`, #340): session create responses now include v2 PTY attach metadata, `pty-ws.v1` subprotocol guidance, observer/controller URLs, `default_role: observer`, and controller policy guidance.
- **AgentCards advertise the real PTY binding** (`00a3233`, #338): `pty-ws/v1` now points at `/agents/{instance_id}/sessions/{session_id}/attach` with implemented replay bounds instead of the old placeholder path.

### Fixed

- **Session identifiers are aligned across HTTP and PTY flows** (`702afdc`, #323): session APIs now consistently return and consume the canonical session id expected by orchestrators.
- **Controller writes are role-gated** (`b6b4ae2`, #325): orchestrator write paths enforce Observer vs Controller authority instead of treating every attach as write-capable.
- **Adapter-command assess mode is allowed** (`89cf5c9`, #326): `adapter-command/v1` can run the provider-free `assess` mode used by the M011 self-guidance adapter smoke.
- **A2A task artifacts are exposed over HTTP** (`6542a57`, #327): completed task artifacts are retrievable through the executor surface instead of being visible only in runtime-local state.
- **VM readiness waits for current agent freshness** (`2ec1da0`, #328): QEMU provisioning no longer accepts stale agent registration as readiness for a newly provisioned VM.
- **VM registered agents are classified correctly** (`a9872c1`, #330): runtime metadata now reports VM-backed A2A instances as VMs rather than falling through as container/default runtime kinds.
- **Evicted PTY output is durably searchable** (`093bc1b`, #337): older PTY frames spill to per-session JSONL transcript files under `pty-transcripts/` and can be searched explicitly beyond the hot replay window.
- **Idle Observer probes can succeed cleanly** (`4ffd8df`, #349): `sandboxctl tui observe --idle-ok` exits 0 after a successful idle Observer attach, while strict timeout behavior remains unchanged without the flag.
- **Interactive session create honors command launch** (`bee1f53`, #352): `POST /api/v1/agents/{agent}/sessions` now launches the requested command inside the named tmux session instead of always opening a generic shell. This enables one-call provider TUI launch.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.7.md` documents the automation-control/TUI orchestration release, verification paths, and known follow-ups.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.7`**.
- **Preferred Codex automation-control launch profile** from live validation: `cd /tmp && TERM=xterm NO_COLOR=1 codex --no-alt-screen`. This avoids the large startup animation in raw tmux capture and reaches the main prompt after update/trust gates.
- **Superseded by v2026.5.8**: tag CI run 565 failed E2E because agentshare was not initialized on titan. Use v2026.5.8 or newer as the clean automation-control/TUI release.
- **Known follow-ups remain open**: #351 tracks `tui search` semantics for hot snapshot text vs durable transcript spill; #353 tracks browser UI reconnect/snapshot corruption under high-redraw provider TUIs.
- **CI status before release prep**: main push workflows 561 and 562 passed on `bee1f53`. Tag CI remains the source of truth for release artifacts.

### Issues closed

- **#314** — A2A task artifacts not retrievable through HTTP.
- **#319** — VM readiness can accept stale agent registration.
- **#320** — adapter-command assess mode should be permitted.
- **#321** — scoped slices toward full end-user TUI sessions as orchestrator-readable/interactable runtimes.
- **#322** — session id contract mismatch.
- **#324** — orchestrator Controller writes need explicit authority gating.
- **#331** — PTY transcript history needs durable searchable spill.
- **#333** — non-PTY event history needs durable spill beyond hot memory.
- **#339** — session create should expose PTY attach metadata.
- **#346** — automation-control loadout blueprint.
- **#348** — idle Observer attach should have a success mode.
- **#350** — interactive create ignored requested command body.


## [2026.5.6] — 2026-05-20

> **A2A routing patch.** One operator-visible bug fix. VM-provisioned agents could register over gRPC and appear in `/api/v1/agents`, but `/agents/{instance_id}/.well-known/agent-card.json` returned `instance.not_found` because the v2/A2A `InstanceRegistry` was only populated by the admin-v2 provision path. v2 routing for VM-backed agents now works the same as Docker admin-v2 instances.

### Fixed

- **gRPC-registered agents now bridge into the v2/A2A `InstanceRegistry`** (`2d09959`, `95f4bea`, #317): `AgentServiceImpl` gained optional `instance_registry` + `signing_keys_dir` fields, wired in `main.rs` whenever the executor surface is mounted. On each `Registration` message, the canonical `instance_id` assigned by `ConnectedAgent::new` (registry.rs:112-116 — client-provided or server-synthesized UUIDv7) gets a matching `InstanceContext` built and inserted into the executor's `InstanceRegistry` via a new `bridge_register_instance` helper. Empty `loadout` → `RuntimeKind::Container` (legacy docker run path), non-empty → `RuntimeKind::Vm` (cloud-init always materializes a loadout). The bridge is idempotent on duplicate `instance_id`, so admin-v2's pre-registration is preserved and the cached AgentCard isn't invalidated when the agent reconnects. On disconnect, the v2 entry is removed before the v1 unregister destroys the id mapping. Discovered during the agent-ops M011 dual-substrate smoke against v2026.5.5.

### Documentation

- **`docs/releases/v2026.5.6.md`**: release announcement covering the routing fix and the M011 reproduction path.

### Operator notes

- **`agentic-mgmt` bumps to `2026.5.6`**; `sandboxctl` and `agent-client` follow. No protocol change — agents built against v2026.5.5 work unchanged against the v2026.5.6 server.
- **Reproduction** of the original bug: with v2026.5.5, `provision-vm.sh --loadout profiles/codex-only.yaml ...` produced a VM that registered in v1 and showed in `/api/v1/agents`, but `GET /agents/<instance_id>/.well-known/agent-card.json` returned 404 `instance.not_found`. After upgrade, the same reproduction returns the signed AgentCard.
- **Tests**: three new unit tests in `management/src/grpc.rs::tests` cover the bridge (VM kind, Container fallback, idempotency). Full suite 516 passed locally; CI gate stays as the source of truth.
- **No data migration** — the registry is in-memory, rebuilt on every server start.

### Issues closed

- **#317** — VM-provisioned agents register in v1 registry but are not routable A2A instances


## [2026.5.5] — 2026-05-20

> **End-to-end validation patch.** Six commits since v2026.5.4 — all from running the v2026.5.4 fixes end-to-end on a real libvirt host and finding what the dry-run validation didn't catch. The build pipeline (#312) and browser-qa loadout (#313) are now genuinely operator-validated, with three new operator-visible bugs fixed along the way. E2E CI is back on the release-blocking path.

### Added

- **`scripts/validate-browser-qa.sh`** (`55df8e1`, `3b063af`, #313): operator helper. Runs over SSH against a provisioned browser-qa VM and checks all seven acceptance criteria — `Xorg :99` running via `xorg99.service`, `/dev/uinput` mode 0660 group `input`, `/opt/carbonyl/carbonyl --version` returns the pinned runtime, `python3 -c "import uinput"` succeeds, `agent` user in `input` group, `xserver-xorg-input-evdev` installed, `xorg99.service` active. Exit 0 on pass, 1 on fail, 2 on SSH-unreachable. Shellcheck clean.

### Fixed

- **`get_health_token_hash` permission regression** (`58c50c6`, follows #259): commit `5ed46b8` (the #259 hotfix) tightened `HEALTH_TOKENS_FILE` from mode 0644 → 0600 owned by root, but `get_health_token_hash()` in `lib/secrets.sh` was doing an unprivileged `grep` against the file. With `set -euo pipefail` in the caller (`provision-vm.sh`), the silent permission-denied exit propagated and aborted every loadout-based provision at "Generating health endpoint token…" without an obvious error. Function now uses `sudo grep`. Discovered while running #313's live-VM validation.
- **`browser-automation.yaml` layer — three issue-body bugs** (`629b598`, #313): live VM validation surfaced bugs that the issue body's proposed YAML had inherited.
  - `xserver-xorg-video-modesetting` does not exist as a standalone package in Ubuntu 24.04 (modesetting driver is built into `xserver-xorg-core`). cloud-init raised `NoPackageError` on the first match and aborted the entire 51-package install run. Removed from the layer.
  - `99-uinput.rules` udev drop-in did not apply retroactively — `/dev/uinput` was created by `modprobe uinput` before the rule landed, so the existing node stayed `crw------- root:root`. Added `udevadm control --reload-rules` + `udevadm trigger /dev/uinput || true` so the existing node picks up `group=input mode=0660` in the same cloud-init pass.
  - No mechanism started `Xorg :99` despite "Xorg :99 runs" being a stated acceptance criterion. Added `/etc/X11/xorg.conf.d/10-dummy-display.conf` (1280x800x24 backed by `xserver-xorg-video-dummy`, matching the carbonyl qa-runner default viewport), `/etc/systemd/system/xorg99.service` (Type=simple, Restart=on-failure), and `systemctl enable --now xorg99.service` to runcmd.
- **`build-base-image.sh` autoinstall no-poweroff** (`b5b1e18`, #312): the `--cdrom` → `--location` switch in `f105c9f` (v2026.5.4) unblocked virt-install acceptance but exposed a second latent bug — autoinstall has no shutdown trigger, so the installer reboots into the installed system and sits idle at a login prompt forever. `virt-install --wait -1` and the subsequent wait-loop in `build_image()` hang on this indefinitely. Validation observed exactly this (VM idle ~10 min post-install with effectively zero CPU activity). Added `- shutdown -h now` to autoinstall late-commands. Future builds self-complete.

### Changed

- **E2E CI hard-gate restored** (`9720215`, #312): reverted the `if: false` workaround from commit `13faf95`. With #312 validated end-to-end, e2e once again gates tag pushes. Following the runbook's two-step path — this is the tag-only restoration; after v2026.5.5+ ships cleanly with e2e green, drop the `if:` entirely so e2e gates every push.

### Documentation

- **`docs/LOADOUTS.md`** (`b6ba53a`, #313): browser-qa table row now points at `scripts/validate-browser-qa.sh` so the verification step has a one-line answer next to the loadout entry.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, `agent-client`** all bump to `2026.5.5`. Loadout-based VM provisioning works again (was silently failing since #259's hotfix); existing VMs are unaffected.
- **The browser-qa loadout is now operator-proven**, not just code-proven. `./images/qemu/provision-vm.sh agent-browser --loadout profiles/browser-qa.yaml --ssh-key <key> --wait-ready` then `./scripts/validate-browser-qa.sh agent-browser` returns 0 — 7/7 acceptance checks passed on titan.
- **`build-base-image.sh 24.04`** is now operator-proven end-to-end. Validated on titan: built a 2.94 GiB sparse qcow2 in ~10 min, virt-customize + virt-sparsify + chmod 444 + chattr +i + manifest sha256 record all clean. The poweroff fix means future runs do not need any manual shutdown.
- **Tag context will exercise the restored e2e gate** for the first time since v2026.5.0. If e2e fails on the v2026.5.5 tag, the release pipeline stops at integration before release-attach; no broken release will publish.

### Issues closed

- **#312** — `build-base-image.sh` virt-install API incompatibility (full chain: v2026.5.4's `f105c9f` + this release's `b5b1e18`, `9720215`)
- **#313** — browser-qa loadout for trusted-input browser automation (full chain: v2026.5.4's `df3ba86` + this release's `58c50c6`, `629b598`, `3b063af`, `b6ba53a`)


## [2026.5.4] — 2026-05-20

> **Security hardening + tooling fix release.** Three commits since v2026.5.3 plus backlog hygiene. Notable change: `LISTEN_ADDR` default flips to loopback, cutting cross-VM lateral access on virbr0 per the documented single-host threat model.

### Security

- **Default `LISTEN_ADDR` to loopback** (`a1baab4`, #256 + #257): `management/src/config.rs` default changed from `0.0.0.0:8120` to `127.0.0.1:8120`. All three management listeners (gRPC `:8120`, WS `:8121`, HTTP `:8122`) derive their bind IP from `grpc_addr.ip()` in `main.rs`, so this single change moves all three onto loopback. Cuts the cross-VM lateral path on virbr0 entirely — VMs cannot reach `127.0.0.1` from their interfaces.
  - **#256** (WS unauth → cross-VM RCE): resolved against the documented threat model. WS bearer-auth-on-upgrade documented as a future follow-up (needs paired dashboard JS work; `management/ui/app.js` currently opens WebSocket connections without an Authorization header).
  - **#257** (gRPC/HTTP/WS plaintext TCP, bearer sniffable on virbr0): resolved against the documented threat model. Full TLS wiring (gRPC `tonic::ServerTlsConfig`, rustls-aware WS accept, axum TLS) remains tracked for multi-host deployments.
  - Operators who explicitly want non-loopback exposure set `LISTEN_ADDR=0.0.0.0:8120` and should configure TLS + bearer/mTLS auth before exposing.

### Added

- **`browser-qa` task-focused loadout** (`df3ba86`, #313): VM-isolation fallback for trusted-input browser QA (carbonyl + uinput + Xorg). Two new manifests:
  - `images/qemu/loadouts/layers/browser-automation.yaml` — composable layer: Xorg evdev, `/dev/uinput` udev rule (`mode 0660 group input`), `python3-uinput`, carbonyl runtime pinned to `runtime-x11-8f070d2720157bd0`, `systemd-udevd` for X hot-plug of runtime-created uinput devices, `usermod -aG input agent`, `modprobe uinput`.
  - `images/qemu/loadouts/profiles/browser-qa.yaml` — full profile (4 cpu / 8G ram / 40G disk / network full). Extends `layers/base-dev.yaml` + `layers/browser-automation.yaml`.
  - Docker isolation via `carbonyl-agent/docker/qa-runner` remains the preferred runtime; this VM profile exists for the case where Docker hot-plug for runtime-created uinput devices is unavailable. See `roctinam/carbonyl-agent#120` for the Docker hot-plug regression that motivates needing the fallback path.
  - End-to-end verified locally: `resolve-manifest.sh` + `generate-from-manifest.sh` → `yaml.safe_load` clean, 51 packages, 15 write_files, 22 runcmd.

### Fixed

- **`build-base-image.sh` `virt-install` API incompatibility** (`f105c9f`, #312): virt-install 1.x (Ubuntu 25.10) rejects `--cdrom` paired with `--extra-args` (`ERROR Kernel arguments are only supported with location or kernel installs.`). Switched to `--location "$iso_path,kernel=casper/vmlinuz,initrd=casper/initrd"` so the autoinstall trigger + serial console kernel args are accepted. The cidata autoinstall ISO remains attached as a second cdrom and is still discovered by cloud-init's NoCloud datasource via the `cidata` volid set in `generate_autoinstall_iso`.
- **Broken CHANGELOG `[Unreleased]` compare link** (this commit): the footer link `[Unreleased]: P26.5.3...HEAD` was malformed (typo, missing `v` prefix and host). Fixed to the canonical GitHub compare URL.

### Documentation

- **`docs/LOADOUTS.md`** — `browser-qa` row added to the Task-Focused table with the carbonyl-agent#120 cross-ref.
- **`management/README.md`** + **`management/dev.sh`** — `LISTEN_ADDR` default documented as `127.0.0.1:8120`; opt-out instructions for non-loopback exposure included.

### Operator notes

- **Default bind change is a behavior change.** Operators running with the implicit default get loopback-only listeners after upgrade. Multi-host or remote-dashboard deployments must set `LISTEN_ADDR=0.0.0.0:8120` (or the appropriate routable bind) explicitly in `/etc/agentic-sandbox/management.env` or via env var.
- **`build-base-image.sh` change** is operator-validated, not CI-validated (#312 thread tracks the titan smoke-test). Re-run the script on a host with libvirt + KVM + the casper-layout Ubuntu live ISO to confirm; report any failure in #312.
- **`browser-qa` loadout** is operator-validated, not CI-validated (#313 thread tracks the libvirt smoke-test). The carbonyl runtime tarball URL is hard-coded; bump in lockstep with `carbonyl-agent/.carbonyl-runtime-version`.

### Backlog hygiene

Audit triage closed four already-resolved issues that had remained open:
- **#258** (Base ISO + qcow2 hash verification): full chain landed in commit `5f936c8` (May 17). Operator follow-up: re-apply `chattr +i` to the existing live `ubuntu-server-24.04-agent.qcow2`.
- **#259** (cloud-init.iso plaintext AGENT_SECRET): hotfix landed in commits `e731838` + `5ed46b8` (May 15, 17); on-disk perms tightened to 0700/0600. SSH-push design work deferred to a future narrowly-scoped issue.
- **#260** (`docker-compose.dev.yaml` mounts `docker.sock`): Option A landed in `97d9e74` (May 17) — bind mount dropped from the obsolete Go-era scaffold.
- **#267** (aiwg_serve logs leak bearer tokens in WS URLs): `redact_ws_url` helper landed in `cc94060` (May 17); 3 unit tests verify.

Five-issue cohort deferred to 2026-08-17 check date: #114 epic (Platform-agnostic VM provisioning with Alpine support) + children #115 (musl build), #118 (Alpine agentic-dev profile), #119 (libvirt/Proxmox backend abstraction), #120 (deploy/lifecycle). Alpine + Proxmox is not a near-term direction; the dependency-free piece (#115, musl build) is ready when scheduling resumes.

## [2026.5.3] — 2026-05-19

> **First artifact-bearing release.** This is the release the v2026.5.1 and v2026.5.2 source-only notices pointed at. The release pipeline now produces versioned binary tarballs (x86_64-linux-gnu + x86_64-linux-musl + aarch64-apple-darwin + aarch64-unknown-linux-gnu) with SHA256SUMS, version-stamped container images, and (when operator secrets are provisioned) cargo publish, multi-registry push, SBOM, and signed artifacts. CI is green on `titan`/`teroknor`/`mutsu` — never on the workstation runner.

Release pipeline went from "creates a release page in 3 seconds, no artifacts" to a full multi-architecture build with explicit gates. The bulk of this release is CI work, plus one runtime-visible dependency swap (rustls).

### Highlights

| What changed | Why you care |
|---|---|
| **Release pipeline produces real artifacts** | Tag push → `prerelease-gate` validates → 4 platform builds run in parallel → tarballs + SHA256SUMS attach to the Gitea release. Aarch64 builds happen on a Mac Mini via SSH-from-Linux-runner. |
| **HTTP + WebSocket stacks switched to rustls** | `reqwest` and `tokio-tungstenite` no longer pull `native-tls` / system OpenSSL. Pure-Rust TLS stack; cleanly cross-compiles. No runtime behavior change for clients. |
| **CI runner re-routing** | Every workflow job now targets `titan` (heavy build) or `teroknor` (light/network) by explicit label. Zero `runs-on: self-hosted` remains — workstation runners stop receiving CI work. |
| **Per-release container tags** | Internal registry now carries `:v<version>` tags on every release alongside `:latest` and `:<sha>`. Pinning to a release is finally possible. |
| **Single-shot version bump tooling** | `scripts/bump-version.sh <version>` updates 3 Cargo.toml + 3 Cargo.lock + inserts new CHANGELOG section + footer link in one command. Replaces the manual edit dance. |

### Added

- **`release-binaries` matrix in `ci.yaml`** (`#297`) — tag-only job that builds `agentic-mgmt`, `agent-client`, `sandboxctl` for `x86_64-unknown-linux-gnu` (full set), and `agent-client` + `sandboxctl` for `x86_64-unknown-linux-musl` (the `management` crate is excluded for musl — `agentic-mgmt` hard-links to system libvirt and no musl-compatible libvirt sysroot exists; same exclusion as aarch64-linux). Packages each as `agentic-sandbox-vX.Y.Z-<arch>-<libc>.tar.gz`, generates per-file `.sha256` sidecars plus an aggregated `SHA256SUMS`, uploads as workflow artifacts.
- **`release-binaries-mutsu` job** — `aarch64-apple-darwin` (native Mac build) and `aarch64-unknown-linux-gnu` (cross-compiled via `cargo-zigbuild`) built by SSHing from a Linux runner to mutsu (Apple M4). Matches the proven `fortemi/publish-sidecar.yml` pattern; avoids the known reverse-proxy / gRPC task-fetch failure mode of native `runs-on: mutsu`. Gated on `MUTSU_SSH_KEY` secret with skip-with-warning when absent. **Both mutsu tarballs exclude `agentic-mgmt`** — it hard-links to libvirt via the `virt` FFI crate, and neither macOS nor aarch64-linux has a usable libvirt sysroot on the build host. Tarballs include a `MGMT_EXCLUDED.txt` note.
- **`release-attach` job** — consolidates release creation into `ci.yaml`. Downloads matrix artifacts, aggregates a canonical `SHA256SUMS`, re-verifies Cargo + CHANGELOG (defense-in-depth), creates the Gitea release, attaches every tarball + checksum file as release assets. Replaces `gitea-release.yaml` (deleted).
- **`prerelease-gate` job** (`#295`) — verifies all three `Cargo.toml` versions match the tag base AND `CHANGELOG.md` has a matching `## [<version>]` section. Tag-only; gates `release-binaries` and `release-binaries-mutsu`.
- **`:v<version>` container tags** (`#305`) — `docker` job now emits `:latest`, `:<sha>`, AND `:v<version>` on tag pushes for all 6 images (mgmt, agent-client, agent, claude, codex, opencode).
- **`tags: ['v*']`** added to `ci.yaml` triggers (`#304`) — the full pipeline now runs against the tag commit, not just the prior branch commit.
- **`cargo-publish` job** (`#296`, secret-gated) — publishes `agent-rs`, `management`, `cli` to crates.io in dep order with `--dry-run` first. Skip-with-warning when `CARGO_REGISTRY_TOKEN` not configured.
- **`multi-registry-push` job** (`#299`, secret-gated per registry) — mirrors all 6 release-tagged images to `ghcr.io/<owner>/*` and `quay.io/<user>/*`. Each registry gates independently on its credentials.
- **`sign-and-sbom` job** (`#300`, secret-gated per capability) — GPG-signs binary tarballs (`.asc` detached), cosign-signs container images, generates per-tarball SBOM (CycloneDX via syft). Each capability gates independently.
- **`github-release-sync` job** (`#306`, secret-gated) — idempotent `gh release create/edit` mirroring the Gitea release to `jmagly/agentic-sandbox` with tarballs + notes.
- **`scripts/bump-version.sh`** (`#301`) — CalVer validation (no leading zeros), dirty-tree refusal, idempotency check, updates 3 Cargo.toml + 3 Cargo.lock, inserts new CHANGELOG section with placeholders, updates Unreleased compare-link and inserts the new version's compare-link.
- **`docs/releases/runbook.md`** — end-to-end release procedure with required-secrets table, rollback procedure, and runner-assignment table.
- **`docs/architecture/release-pipeline-audit.md`** — full inventory of every `.gitea/workflows/*.{yml,yaml}` workflow, ASCII diagram of the tag-push flow, 4-phase remediation plan, and acceptance criteria for a "fixed" pipeline.
- **`docs/architecture/aarch64-build-runner-plan.md`** — mutsu (Mac Mini) inventory, three architectural options (native Mac + cross-build / Linux VM on Mac / port runtime to macOS), recommendation, and bootstrap procedure.
- **Ubuntu 24.04.3 pinned in `iso-pins.json`** — sha256 verified against the GPG-signed `SHA256SUMS` from `releases.ubuntu.com`.

### Changed

- **HTTP client stack: `reqwest` switched from `native-tls` to `rustls`** (`#311`, commit `c39c6c9`). `cli`, `management`, and `agentic-sandbox-executor` now use `reqwest = { default-features = false, features = ["json", "rustls-tls"] }`. tonic 0.12's `tls` feature was already rustls-backed — no change there.
- **WebSocket client: `tokio-tungstenite` switched from `native-tls` to `rustls-tls-webpki-roots`** (commit `c39c6c9`). Drops the implicit system OpenSSL dep that blocked aarch64-linux cross-compile.
- **`agentic-sandbox-executor` pins `openssl = { version = "0.10", features = ["vendored"] }`** (commit `8c03411`) — josekit hard-depends on openssl for JOSE primitives. The vendored feature compiles OpenSSL from source as part of the build (~30s overhead per cold build), which lets `cargo zigbuild` cross-compile cleanly to aarch64-linux.
- **All CI workflows re-routed off `runs-on: self-hosted`** (commit `898bad7`). Every job in every workflow file now targets `titan` (heavy: build, docker, e2e, cosign) or `teroknor` (light: validation, network, SSH out) by explicit label. The workstation runner (`grissom`) is excluded from CI by design.
- **`gitea-release.yaml` deleted** — its responsibility is now `release-attach` inside `ci.yaml`. Single linear workflow instead of `workflow_run` cross-workflow handoff.
- **`executor-build.yml` deleted** (`#308`) — `Makefile test-unit` updated to `cargo test --workspace` so executor-crate coverage flows through normal `ci.yaml test`.
- **`docsite-deploy.yml` `push.tags: ['v*']` trigger re-enabled** (`#307`) with secret guards on every step; missing secrets → skip with warning.
- **Lint job moved from `teroknor` to `titan`** (commit `2ec9f4e`) — `cargo fmt --check` needs the Rust toolchain.
- **E2E job conditional**: `if: false` — skipped on every push (branch AND tag) until [#312](https://github.com/jmagly/agentic-sandbox/issues/312) ships and the Ubuntu 24.04 qcow2 is staged on titan. This is a temporary workaround so v2026.5.3 (and any patch releases between now and #312) can ship without the broken-bootstrap blocker. When #312 lands, restore: first `if: startsWith(github.ref, 'refs/tags/v')` for a tag-only gate, then drop the `if:` entirely.
- **README + getting-started clone URL switched** to the GitHub mirror in v2026.5.2; carried forward here.

### Fixed

- **`build/docker` skip-on-branch regression** (commit `6928b7d`) — Phase 1 (#295) added `prerelease-gate` to their `needs:` list. `prerelease-gate` is tag-only, and Gitea/GitHub Actions propagate skipped needs as skips downstream. Removed `prerelease-gate` from `build` and `docker`; the release-* jobs that genuinely need the gate (and are themselves tag-only) keep it.
- **`actions/setup-python@v5.6.0` has no prebuilt for Ubuntu 25.10** (titan's OS, commit `e5497e5`). Dropped the action; e2e now uses titan's system Python 3.13 in a `/tmp/e2e-venv` venv (PEP 668 compliant).
- **`pin-iso.sh` fingerprint regex** (commit `5af3b88`) — gpg formats the 40-char fingerprint as two halves of 5 hex-groups separated by **two** spaces (e.g. `B374  2BC0`). The original `([A-F0-9]{4} ){9}[A-F0-9]{4}` regex required single spaces and silently captured an empty `signer_fp`, causing the script to abort without writing the pinned sha256.
- **`release-binaries` packaging step**: honors `$CARGO_TARGET_DIR` (set on mutsu via launchd env) when present; falls back to per-crate `<crate>/target/` otherwise. Uses `sha256sum 2>/dev/null || shasum -a 256` so macOS (no GNU `sha256sum`) works alongside Linux.

### Documentation

- New: `docs/releases/runbook.md`, `docs/architecture/release-pipeline-audit.md`, `docs/architecture/aarch64-build-runner-plan.md` (see Added).
- `docs/releases/runbook.md` extended with a **CI runner assignments** table mapping each runner to the work it gets (`titan` for heavy, `teroknor` for light, `grissom` explicitly excluded) and a **Required secrets** table mapping each secret to the job it activates.
- `docs/architecture/release-pipeline-audit.md` Phase 1-4 status flipped to **landed** with per-issue commit references.
- `docs/architecture/aarch64-build-runner-plan.md` updated to reflect the switch from native act_runner to the SSH-from-Linux-runner pattern and the cleanup of the act_runner registration.

### Removed

- `gitea-release.yaml` — consolidated into `ci.yaml release-attach`.
- `executor-build.yml` — covered by `cargo test --workspace` in the main test job.
- mutsu `act_runner` registration (id 15) — workflow now uses SSH-from-Linux pattern instead. LaunchAgent + `~/Library/Application Support/agentic-sandbox-runner/` removed; toolchain under `/Volumes/build/agentic-sandbox/` (Rust + zig + protoc + cargo-zigbuild) kept for the SSH builds.

### Required secrets (new this release)

The new release jobs are wired but skip-with-warning until provisioned. Provision in **Repo Settings → Actions → Secrets**:

| Secret(s) | Activates |
|---|---|
| `MUTSU_SSH_KEY` | aarch64 builds via `release-binaries-mutsu` |
| `CARGO_REGISTRY_TOKEN` | `cargo-publish` |
| `GHCR_TOKEN` and/or `QUAY_USERNAME`+`QUAY_PASSWORD` | multi-registry container push |
| `COSIGN_KEY`+`COSIGN_PASSWORD` and/or `GPG_PRIVATE_KEY`+`GPG_PASSPHRASE` | container/tarball signatures + SBOM |
| `GITHUB_MIRROR_TOKEN` | GitHub Releases sync |
| `GT_ACCESS_TOKEN`, `DEPLOY_SSH_KEY`, `DEPLOY_HOST`, `DEPLOY_PORT`, `DEPLOY_USER`, `DEPLOY_PATH` | docsite-deploy (issue [#194](https://github.com/jmagly/agentic-sandbox/issues/194)) |

### Operator notes

- **No runtime behavior change for v1 or v2 clients.** The rustls swap is internal — TLS handshakes succeed against the same servers, with the same cipher suites in practice. webpki-roots bundles the Mozilla CA list; system trust store is no longer consulted.
- **Build environment changed.** Compile-from-source builds now require the openssl C source compile pass (~30s once, cached after) due to josekit. `cargo build --release` from the repo root continues to work.
- **CI runner provisioning** (one-time, completed on titan during this release): `libvirt-dev`, `libguestfs-tools`, `golang-go`, `python3-venv` installed via passwordless `sudo apt-get`. Documented in the pipeline-audit doc for future reproducibility.
- **E2E on branch pushes is skipped** until [#312](https://github.com/jmagly/agentic-sandbox/issues/312) lands (build-base-image.sh virt-install fix + base image staged on titan). Tag pushes still gate hard on e2e.
- **Tag this release with the new tooling**: `scripts/bump-version.sh` already ran for this changelog entry. Step 4-5 of `docs/releases/runbook.md` covers `git tag -a v2026.5.3 -m '...'` and the push.


## [2026.5.2] — 2026-05-19

> **Source-only release.** Same caveat as v2026.5.1: no version-stamped binaries, container images, or SBOMs are attached. Build from source via `make build` (release commit recorded on the tag). Release-artifact CI is tracked under [#295](https://github.com/jmagly/agentic-sandbox/issues/295), [#297](https://github.com/jmagly/agentic-sandbox/issues/297), [#299](https://github.com/jmagly/agentic-sandbox/issues/299), [#300](https://github.com/jmagly/agentic-sandbox/issues/300), [#304](https://github.com/jmagly/agentic-sandbox/issues/304), [#305](https://github.com/jmagly/agentic-sandbox/issues/305) and will land before the first artifact-bearing release.

Three-commit patch release following v2026.5.1. Focus: a conformance-CI stability fix that surfaced under self-hosted runner load, plus the post-v2026.5.1 release-pipeline audit and the README clone-URL switch.

### Changed

- **`gitea-release.yaml` reality marked source-only in CHANGELOG and release announcement.** The v2026.5.1 release was cut without artifact-build wiring; the previous entry now states this plainly and links the follow-on CI issues. (`f012773`)
- **README + getting-started clone URL switched to the GitHub mirror.** Internal Gitea remains the authoritative issue tracker for maintainers; public-facing docs show the GitHub URL. (`d25e1fc`)

### Fixed

- **Conformance harness no longer fails CI on transient rustc SIGSEGV under runner contention.** `conformance.yml` now serializes runs per ref, caps stack/build job parallelism, logs Rust/Cargo metadata, and retries Rust-build failures *only* when the failure matches a compiler-crash signature — once, with serialized jobs. Functional test failures still fail fast. ([#309](https://github.com/jmagly/agentic-sandbox/issues/309), `1c2cc33`)

### Documentation

- **New: `docs/architecture/release-pipeline-audit.md`** — full inventory of the 8 `.gitea/workflows/*.{yml,yaml}` files, exactly what runs on a tag push today (≈3s, no artifacts), a 4-phase remediation plan, and explicit acceptance criteria for what a "fixed" release pipeline must produce. ([`f012773`](https://github.com/jmagly/agentic-sandbox/commit/f012773))
- **Source-only notices on v2026.5.1.** CHANGELOG `[2026.5.1]` heading and `docs/releases/v2026.5.1.md` both gained an explicit "source-only" notice; the live Gitea release body was updated in-place to match.

### Issues filed during the audit

Five gaps not previously tracked were filed against the release pipeline:

- [#304](https://github.com/jmagly/agentic-sandbox/issues/304) — `ci.yaml` triggers on `v*` tag pushes (P1, co-requisite for #295)
- [#305](https://github.com/jmagly/agentic-sandbox/issues/305) — internal registry `:v<version>` container tags (P1, co-requisite for #299)
- [#306](https://github.com/jmagly/agentic-sandbox/issues/306) — sync Gitea releases to GitHub mirror Releases page (P2)
- [#307](https://github.com/jmagly/agentic-sandbox/issues/307) — re-enable `docsite-deploy.yml` on `v*` tag pushes (P2)
- [#308](https://github.com/jmagly/agentic-sandbox/issues/308) — fold `executor-build.yml` into `ci.yaml` (P3, cleanup)

### Operator notes

- No code paths changed; no behavior change for v1 or v2 clients.
- The bar for the *next* release (anything past v2026.5.2) is documented in `docs/architecture/release-pipeline-audit.md` § Acceptance: CI green on the tag commit, binary tarballs + SHA256SUMS, `:v<version>` container tags, cargo publish, SBOM + signatures. Releases that fall short MUST carry the source-only notice.

## [2026.5.1] — 2026-05-19

> **Source-only release.** This release ships from source. No version-stamped
> binaries, container images, or SBOMs are attached to the release page.
> Container images on the internal registry are tagged `:latest` and
> `:<git-sha>` only; pull `ef61337c4f` for the release commit, or build
> from source via `make build`. Release-artifact CI lands in a follow-up
> release; see issues
> [#295](https://github.com/jmagly/agentic-sandbox/issues/295) (pre-release gate),
> [#297](https://github.com/jmagly/agentic-sandbox/issues/297) (binary tarballs + checksums),
> [#299](https://github.com/jmagly/agentic-sandbox/issues/299) (release-tagged container push),
> [#300](https://github.com/jmagly/agentic-sandbox/issues/300) (signatures + SBOM).

First CalVer cut that ships the v2 (A2A-aligned) executor surface GA, alongside a full security-hardening pass, the v2 dashboard, and the AIWG executor bridge. v1 remains fully operational with Sunset headers.

> **Versioning.** This release closes out the v2.0 contract work begun under the placeholder `[2.0.0]` section below — that section describes the *contract*; this section describes the **shipped CalVer release** that first carries it.

### Highlights

| What changed | Why you care |
|---|---|
| **v2 executor surface (GA)** | Three-surface split — admin, A2A per-instance, observability. AgentCard discovery, JCS+Ed25519 signing, five A2A extensions (`runtime/v1`, `idempotency/v1`, `hitl-prompt/v1`, `multi-tenant/v1`, `adapter-command/v1`). |
| **v1 → v2 compatibility shim** | Every v1 response now carries `Sunset`, `Deprecated`, `Link` headers. v1 stays live; clients can discover v2 without out-of-band knowledge. Removal targets v3.0, no earlier than 2027-05-09. |
| **AIWG executor bridge** | `agentic-sandbox` can register itself as an executor with an `aiwg serve` instance and accept mission dispatches over WebSocket. SQLite-backed task store + idempotency cache, persistence across restarts, resumable missions. |
| **v2 dashboard** | Sidebar v1→v2 admin migration, signed AgentCard view per instance, extension activation chips per task, push-notification CRUD UI, HITL prompt envelope rendering, Sunset banner. |
| **Security hardening pass** | SHA-pinned all CI actions, digest-pinned all Dockerfiles, dropped root in deploy images, pinned npm installs, constant-time secret comparison, bearer-token log redaction, tightened cloud-init perms. |
| **Conformance harness** | New `roctinam/agentic-sandbox-conformance` test suite wired into CI, plus an end-to-end VM-backed delivery gate that blocks releases on e2e failures. |
| **New getting-started guide** | [`docs/getting-started.md`](docs/getting-started.md) — 15-minute walkthrough with prerequisite verification, container-runtime quick path, VM path, and direct-CLI path. |

### Added

- **A2A executor crate (`agentic-sandbox-executor`)** — A2A core types, AgentCard signer (JWS over JCS-canonical JSON, Ed25519), per-instance router, push-notification handlers. (#234–#243, #245, #252, #253)
- **A2A REST surface** — full message/task lifecycle under `/agents/{id}/v1/...`: `messages:send`, `tasks/{tid}`, list+filter+pagination, cancel, SSE subscribe, `extendedAgentCard`, pushNotificationConfigs CRUD.
- **`pty-ws/v1` binding** — A2A-compatible PTY transport at `wss://host/agents/{id}/sessions/{sid}/attach`; spec under `docs/contracts/bindings/pty-ws/v1/`.
- **AgentCard discovery** at `/agents/{id}/.well-known/agent-card.json` — JCS canonicalization, JWS signature, declared `supportedInterfaces`, `securitySchemes`, and v2.0 extensions.
- **Five A2A extensions** (ADR-019): `runtime/v1`, `idempotency/v1`, `hitl-prompt/v1`, `multi-tenant/v1` (beta), `adapter-command/v1`.
- **AIWG executor bridge** (#193, four passes) — registers with `aiwg serve`, accepts mission dispatches via `POST /api/v1/sessions/:id/dispatch`, pushes the full `mission.*` event vocabulary back over `/ws/executors/{id}`. SQLite TaskStore + IdempotencyCache (Wave 2 W2.1/W2.2). v1 missions.json → v2 missions.db migration tool (W2.3). Exit-code semantics, persistence, resumability (close of #193 deferred gaps).
- **v2 admin API** with mTLS / unix-peer-creds auth (#238, #239) — real provisionInstance, instance lifecycle, integrated with InstanceRegistry.
- **`sandboxctl` v2** (#251) — v2 admin migration, A2A task verbs, AgentCard signature verification.
- **Per-instance Ed25519 signing keys** persisted across restarts (#253).
- **v2 dashboard rewrite** (#244–#250):
  - Sidebar migrated from v1 admin to v2 via `ApiClient` wrapper.
  - Signed AgentCard panel per instance.
  - A2A extension activation chips with per-task filter.
  - PTY view bound to `pty-ws/v1` (multi-controller, replay, keyframes).
  - HITL prompt envelope rendering on `INPUT_REQUIRED` tasks (read-only).
  - Push-notification config CRUD UI per task.
  - Sunset banner with hit count and Settings → Deprecation panel.
- **`adapter-command/v1` extension** for bounded plan-mode dispatch.
- **Idempotency hit counter** + admin OpenAPI coverage lint in CI.
- **VM image integrity verification** end-to-end (#258) — ISO + qcow2 checksums verified at every provision step.
- **Conformance harness in CI** — new `roctinam/agentic-sandbox-conformance` suite wired up (Wave 5 W5.4), including auth coverage for executor routes and JWKS handling.
- **VM-backed delivery gate** — `run-e2e-tests.sh` hardened; CI now blocks delivery on e2e failures, kills orphan mgmt servers, resets runtime state between conformance and e2e.
- **Docsite build/deploy workflows** (`ci(docs)`) and architecture-refs / sub-crate READMEs / welcome / glossary / concepts (#224–#233).
- **`docs/getting-started.md`** — dedicated 15-minute walkthrough with prerequisite verification one-liner, container-runtime quick path, VM path, direct-CLI path, troubleshooting table.
- **`docs/aiwg-executor.md`** and **`docs/v2-migration-guide.md`** — executor contract integration + v1→v2 migration reference.
- **`docs/testing/conformance-testing.md`** — operator protocol for running the conformance harness locally.

### Security

- **SHA-pinned all `.gitea/workflows/` action references** and container `image:` references (digest pinning), eliminating floating-tag supply-chain risk.
- **Dockerfiles digest-pinned**; deploy images drop root.
- **All `npm install -g` invocations pinned** (supply-chain hardening).
- **Constant-time hash comparison** in `SecretStore::verify` (timing-attack hardening).
- **Bearer tokens redacted** in WS URL logging (#267).
- **Cloud-init secrets, `vm-info.json`, virtiofs mount flags** tightened (#259) — mode 0400, owner-only, no group/world readable.
- **`docker.sock` bind mount removed** from dev compose (#260).
- **A2A-rs deps switched to HTTPS** so Docker builds without SSH key access.
- **2026-05-15 security audit** findings documented under `docs/security/`; all remediation issues filed and resolved.

### Fixed

- `pty_resize` 1/4-screen regression fully resolved (terminal sizing was correct as of 2026.5.0; this release lands the remaining buffer-rebind cases observed under multi-controller load).
- `dispatch messages:send` routes to the runtime correctly; `list_tasks` is now properly instance-scoped (no cross-instance leakage).
- Task `working → completed/failed` driven by the dispatch observer, not by polling.
- A2A task instance index migrated after column add (zero-downtime schema bump).
- Agent `stdin_task` aborts cleanly instead of deadlocking on join.
- Docker provisioning produces usable A2A instances under v2 admin (#252).
- `libvirt`-degraded sidebar fallback (#189) — surfaces gRPC-connected agents when `/api/v1/vms` is unresponsive.
- Conformance harness reaches green: pre-registers instances, aliases paths, aligns runtime params with spec, passes `--jwks` correctly, covers executor routes with auth.
- CI stability: conformance workflow working directory, server lifetime across step boundaries, orphan mgmt-server cleanup, Trivy panic tolerance, `upload-artifact@v3` pin, Spectral ruleset config.
- E2E delivery gate hardened — VM startup verification, agent-deploy retries, resource-limit assertions stabilized.
- `adapter-command/v1` gated on workspace presence; `gitea-release.yaml` no longer hard-fails when the docker context lacks a workspace mount.

### Documentation

- **Restructured README Quick Start** around the dashboard, surfaced the CLI parity flow, and added a prominent link to the new Getting Started guide.
- **Fixed 36 broken intra-doc links** across the docs/ tree.
- **API, CLI, WS-protocol docs synced** with code (one-pass code-to-docs reconciliation).
- **Platform-support matrix** added, plus per-crate READMEs.
- **Promoted architecture references to `docs/`**, excluded `research/`, audited orphan dirs.
- **Subsystem references** added for container runtime, PTY rendering, observability (#225, #226, #227).
- **Contracts dir** (`docs/contracts/`) — Wave 1 v2 contract specs, schema-lint CI, upstream sync workflow for A2A + a2a-rs mirrors.
- **Welcome / glossary / concepts** refreshed; AIWG.md synced to 2026.5.7; positioning doc added.

### Removed

- **Python SDK** (`sdk/python/`) — alpha, unmaintained since inception, never published. Use the REST API directly or the Rust `sandboxctl` CLI.
- **Legacy Python agent runtime** (`agent/`) — deprecated 2026-01-26; superseded by `agent-rs/` (Rust). The README explicitly said "do not modify or extend"; deletion finishes that decision.
- **Orphaned utility scripts** — `scripts/apply-resource-limits-patch.py`, `scripts/update-provision-vm-resource-limits.py`, `scripts/secured-health-server.py`, root `send_command.py` / `test_ws_command.py`, and `images/qemu/checkin-server.py`. Zero live callers.

Remaining Python in-tree is intentional and scoped to `tests/e2e/` (pytest harness driving the CI conformance + delivery gates), which is slated for a Rust port as follow-on work. The live `/api/v1/events` producer now ships as the Rust `vm-event-bridge` binary.

### Deferred

- **CI/packaging publish work** filed as follow-on issues (`cargo publish` for the three Rust crates, multi-registry container push to ghcr + Quay, signed release tarballs + SBOM, pre-release validation gate, automated version bumping). The current release ships from source; binary artifact publishing lands in a follow-up release.
- **Rust port of `tests/e2e/`** — the pytest harness will be replaced once an equivalent Rust integration suite exists. Tracked: [#302](https://github.com/jmagly/agentic-sandbox/issues/302).

### Operator notes

- **No breaking changes** for v1 clients. v1 routes continue to respond identically; the only observable change is the addition of `Sunset` / `Deprecated` / `Link` response headers. v1 removal target: v3.0, no earlier than 2027-05-09 (overridable via `AIWG_V1_SUNSET_DATE`).
- **VMs provisioned before this release** still register and run; pick up the tightened cloud-init perms on re-provision.
- **AIWG bridge consumers** require a sandbox running this version or later for `replayCapable` to flip true.
- **Conformance harness** is required-green for delivery; merging to `main` will not produce release artifacts until the e2e and conformance gates pass.

## [2.0.0] — 2026-05-19 (shipped under CalVer [2026.5.1])

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
  [`roctinam/agentic-sandbox-conformance`](https://github.com/jmagly/agentic-sandbox-conformance)).
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

- [ADR-018 — A2A as base protocol](https://github.com/jmagly/agentic-sandbox/blob/main/.aiwg/architecture/adr/ADR-018-a2a-as-base-protocol.md)
- [ADR-019 — Extension URI scheme and governance](https://github.com/jmagly/agentic-sandbox/blob/main/.aiwg/architecture/adr/ADR-019-extension-uri-scheme-and-governance.md)
- [ADR-020 — PTY custom protocol binding](https://github.com/jmagly/agentic-sandbox/blob/main/.aiwg/architecture/adr/ADR-020-pty-custom-protocol-binding.md)
- [ADR-021 — `a2a-rs` as wire dependency](https://github.com/jmagly/agentic-sandbox/blob/main/.aiwg/architecture/adr/ADR-021-a2a-rs-as-wire-dependency.md)
- [ADR-022 — Three-surface architecture](https://github.com/jmagly/agentic-sandbox/blob/main/.aiwg/architecture/adr/ADR-022-three-surface-architecture.md)

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

[Unreleased]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.12...HEAD
[2026.7.12]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.11...v2026.7.12
[2026.7.11]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.10...v2026.7.11
[2026.7.10]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.9...v2026.7.10
[2026.7.9]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.8...v2026.7.9
[2026.7.8]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.7...v2026.7.8
[2026.7.7]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.6...v2026.7.7
[2026.7.6]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.5...v2026.7.6
[2026.7.5]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.4...v2026.7.5
[2026.7.4]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.2...v2026.7.4
[2026.7.3]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.2...v2026.7.3
[2026.7.2]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.1...v2026.7.2
[2026.7.1]: https://github.com/jmagly/agentic-sandbox/compare/v2026.7.0...v2026.7.1
[2026.7.0]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.36...v2026.7.0
[2026.6.36]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.35...v2026.6.36
[2026.6.35]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.34...v2026.6.35
[2026.6.34]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.33...v2026.6.34
[2026.6.33]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.32...v2026.6.33
[2026.6.32]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.31...v2026.6.32
[2026.6.31]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.30...v2026.6.31
[2026.6.30]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.29...v2026.6.30
[2026.6.29]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.28...v2026.6.29
[2026.6.28]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.27...v2026.6.28
[2026.6.27]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.26...v2026.6.27
[2026.6.26]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.25...v2026.6.26
[2026.6.25]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.24...v2026.6.25
[2026.6.24]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.23...v2026.6.24
[2026.6.23]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.22...v2026.6.23
[2026.6.22]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.21...v2026.6.22
[2026.6.21]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.20...v2026.6.21
[2026.6.20]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.19...v2026.6.20
[2026.6.19]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.18...v2026.6.19
[2026.6.18]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.17...v2026.6.18
[2026.6.17]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.16...v2026.6.17
[2026.6.16]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.15...v2026.6.16
[2026.6.15]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.14...v2026.6.15
[2026.6.14]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.13...v2026.6.14
[2026.6.13]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.12...v2026.6.13
[2026.6.12]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.11...v2026.6.12
[2026.6.11]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.10...v2026.6.11
[2026.6.10]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.9...v2026.6.10
[2026.6.9]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.8...v2026.6.9
[2026.6.8]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.7...v2026.6.8
[2026.6.7]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.6...v2026.6.7
[2026.6.6]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.5...v2026.6.6
[2026.6.5]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.4...v2026.6.5
[2026.6.4]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.3...v2026.6.4
[2026.6.3]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.2...v2026.6.3
[2026.6.2]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.1...v2026.6.2
[2026.6.1]: https://github.com/jmagly/agentic-sandbox/compare/v2026.6.0...v2026.6.1
[2026.6.0]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.17...v2026.6.0
[2026.5.17]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.16...v2026.5.17
[2026.5.16]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.15...v2026.5.16
[2026.5.15]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.14...v2026.5.15
[2026.5.14]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.13...v2026.5.14
[2026.5.13]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.12...v2026.5.13
[2026.5.12]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.11...v2026.5.12
[2026.5.11]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.10...v2026.5.11
[2026.5.10]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.9...v2026.5.10
[2026.5.9]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.8...v2026.5.9
[2026.5.8]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.7...v2026.5.8
[2026.5.7]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.6...v2026.5.7
[2026.5.6]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.5...v2026.5.6
[2026.5.5]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.4...v2026.5.5
[2026.5.4]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.3...v2026.5.4
[2026.5.3]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.2...v2026.5.3
[2026.5.2]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.1...v2026.5.2
[2026.5.1]: https://github.com/jmagly/agentic-sandbox/compare/v2026.5.0...v2026.5.1
[2.0.0]: ./docs/v2-migration-guide.md
[2026.5.0]: https://github.com/jmagly/agentic-sandbox/releases/tag/v2026.5.0
