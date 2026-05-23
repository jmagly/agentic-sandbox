# Changelog

All notable changes to **agentic-sandbox** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Calendar Versioning (CalVer)](https://calver.org/) in
the form `YYYY.M.PATCH` (e.g. `2026.5.0`).

## [Unreleased]

## [2026.5.10] — 2026-05-23

> **Release E2E diagnostics and cleanup patch.** This release supersedes v2026.5.9, whose tag workflow created artifacts but still failed the release-blocking E2E job while waiting for first-boot VM SSH. It keeps the v2026.5.9 readiness fixes and adds actionable VM diagnostics, earlier auto-VM cleanup registration, and a longer tag E2E SSH window.

### Fixed

- **Provisioning-failure diagnostics and cleanup** (#358): `scripts/run-e2e-tests.sh` now marks auto-created E2E VMs for cleanup before invoking reprovisioning, and failed provision/readiness paths emit bounded `virsh`, VM metadata, SSH-key presence, DHCP, and QEMU-log diagnostics.
- **Tag E2E first-boot SSH window** (#356): the release E2E workflow now sets `AGENTIC_VM_SSH_WAIT_SECONDS=900` and `E2E_VM_READY_TIMEOUT=900`, keeping local defaults shorter while allowing slower CI first boots to complete or produce diagnostics.

### Documentation

- **Release announcement**: `docs/releases/v2026.5.10.md` documents the E2E diagnostics/cleanup patch and superseded v2026.5.9 tag.

### Operator notes

- **`agentic-mgmt`, `sandboxctl`, and `agent-client` bump to `2026.5.10`**.
- **v2026.5.9 is superseded**: the release artifacts were created, but tag CI run 583 failed the release-blocking E2E job while waiting for SSH on the first-boot VM. Use v2026.5.10 or newer as the clean automation-control/TUI release.
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
- **v2026.5.8 is superseded**: the release artifacts were created, but tag CI run 578 failed the release-blocking E2E job while waiting for SSH on the first-boot VM. Use v2026.5.10 or newer as the clean automation-control/TUI release.
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
- **v2026.5.7 is superseded**: the release page/artifacts were created, but tag CI run 565 failed E2E because agentshare was not initialized on titan. Use v2026.5.10 or newer as the clean automation-control/TUI release.
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
- **Broken CHANGELOG `[Unreleased]` compare link** (this commit): the footer link `[Unreleased]: P26.5.3...HEAD` was malformed (typo, missing `v` prefix and host). Fixed to the canonical Gitea compare URL.

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
- **E2E job conditional**: `if: false` — skipped on every push (branch AND tag) until [#312](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/312) ships and the Ubuntu 24.04 qcow2 is staged on titan. This is a temporary workaround so v2026.5.3 (and any patch releases between now and #312) can ship without the broken-bootstrap blocker. When #312 lands, restore: first `if: startsWith(github.ref, 'refs/tags/v')` for a tag-only gate, then drop the `if:` entirely.
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
| `GT_ACCESS_TOKEN`, `DEPLOY_SSH_KEY`, `DEPLOY_HOST`, `DEPLOY_PORT`, `DEPLOY_USER`, `DEPLOY_PATH` | docsite-deploy (issue [#194](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/194)) |

### Operator notes

- **No runtime behavior change for v1 or v2 clients.** The rustls swap is internal — TLS handshakes succeed against the same servers, with the same cipher suites in practice. webpki-roots bundles the Mozilla CA list; system trust store is no longer consulted.
- **Build environment changed.** Compile-from-source builds now require the openssl C source compile pass (~30s once, cached after) due to josekit. `cargo build --release` from the repo root continues to work.
- **CI runner provisioning** (one-time, completed on titan during this release): `libvirt-dev`, `libguestfs-tools`, `golang-go`, `python3-venv` installed via passwordless `sudo apt-get`. Documented in the pipeline-audit doc for future reproducibility.
- **E2E on branch pushes is skipped** until [#312](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/312) lands (build-base-image.sh virt-install fix + base image staged on titan). Tag pushes still gate hard on e2e.
- **Tag this release with the new tooling**: `scripts/bump-version.sh` already ran for this changelog entry. Step 4-5 of `docs/releases/runbook.md` covers `git tag -a v2026.5.3 -m '...'` and the push.


## [2026.5.2] — 2026-05-19

> **Source-only release.** Same caveat as v2026.5.1: no version-stamped binaries, container images, or SBOMs are attached. Build from source via `make build` (release commit recorded on the tag). Release-artifact CI is tracked under [#295](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/295), [#297](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/297), [#299](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/299), [#300](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/300), [#304](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/304), [#305](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/305) and will land before the first artifact-bearing release.

Three-commit patch release following v2026.5.1. Focus: a conformance-CI stability fix that surfaced under self-hosted runner load, plus the post-v2026.5.1 release-pipeline audit and the README clone-URL switch.

### Changed

- **`gitea-release.yaml` reality marked source-only in CHANGELOG and release announcement.** The v2026.5.1 release was cut without artifact-build wiring; the previous entry now states this plainly and links the follow-on CI issues. (`f012773`)
- **README + getting-started clone URL switched to the GitHub mirror.** Internal Gitea remains the authoritative issue tracker for maintainers; public-facing docs show the GitHub URL. (`d25e1fc`)

### Fixed

- **Conformance harness no longer fails CI on transient rustc SIGSEGV under runner contention.** `conformance.yml` now serializes runs per ref, caps stack/build job parallelism, logs Rust/Cargo metadata, and retries Rust-build failures *only* when the failure matches a compiler-crash signature — once, with serialized jobs. Functional test failures still fail fast. ([#309](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/309), `1c2cc33`)

### Documentation

- **New: `docs/architecture/release-pipeline-audit.md`** — full inventory of the 8 `.gitea/workflows/*.{yml,yaml}` files, exactly what runs on a tag push today (≈3s, no artifacts), a 4-phase remediation plan, and explicit acceptance criteria for what a "fixed" release pipeline must produce. ([`f012773`](https://git.integrolabs.net/roctinam/agentic-sandbox/commit/f012773))
- **Source-only notices on v2026.5.1.** CHANGELOG `[2026.5.1]` heading and `docs/releases/v2026.5.1.md` both gained an explicit "source-only" notice; the live Gitea release body was updated in-place to match.

### Issues filed during the audit

Five gaps not previously tracked were filed against the release pipeline:

- [#304](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/304) — `ci.yaml` triggers on `v*` tag pushes (P1, co-requisite for #295)
- [#305](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/305) — internal registry `:v<version>` container tags (P1, co-requisite for #299)
- [#306](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/306) — sync Gitea releases to GitHub mirror Releases page (P2)
- [#307](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/307) — re-enable `docsite-deploy.yml` on `v*` tag pushes (P2)
- [#308](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/308) — fold `executor-build.yml` into `ci.yaml` (P3, cleanup)

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
> [#295](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/295) (pre-release gate),
> [#297](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/297) (binary tarballs + checksums),
> [#299](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/299) (release-tagged container push),
> [#300](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/300) (signatures + SBOM).

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

Remaining Python in-tree is intentional and scoped: `tests/e2e/` (pytest harness driving the CI conformance + delivery gates) and `scripts/vm-event-bridge.py` (live producer for `/api/v1/events`, with systemd unit). Both are slated for Rust port as follow-on work.

### Deferred

- **CI/packaging publish work** filed as follow-on issues (`cargo publish` for the three Rust crates, multi-registry container push to ghcr + Quay, signed release tarballs + SBOM, pre-release validation gate, automated version bumping). The current release ships from source; binary artifact publishing lands in a follow-up release.
- **Rust port of `scripts/vm-event-bridge.py`** — the last load-bearing Python in the runtime path. Tracked: [#303](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/303).
- **Rust port of `tests/e2e/`** — the pytest harness will be replaced once an equivalent Rust integration suite exists. Tracked: [#302](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/302).

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

[Unreleased]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.10...HEAD
[2026.5.10]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.9...v2026.5.10
[2026.5.9]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.8...v2026.5.9
[2026.5.8]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.7...v2026.5.8
[2026.5.7]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.6...v2026.5.7
[2026.5.6]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.5...v2026.5.6
[2026.5.5]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.4...v2026.5.5
[2026.5.4]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.3...v2026.5.4
[2026.5.3]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.2...v2026.5.3
[2026.5.2]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.1...v2026.5.2
[2026.5.1]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.0...v2026.5.1
[2.0.0]: ./docs/v2-migration-guide.md
[2026.5.0]: https://git.integrolabs.net/roctinam/agentic-sandbox/releases/tag/v2026.5.0
