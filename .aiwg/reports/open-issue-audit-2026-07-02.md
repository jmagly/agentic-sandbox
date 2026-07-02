# Open Issue Audit - 2026-07-02

Scope: Gitea `roctinam/agentic-sandbox` tracker, all open issues visible via `mcp__git_gitea.list_issues` on the `main` branch after commit `de2378c`.

Mode: backlog audit and implementation-readiness triage. No issue state was changed by this audit.

## Snapshot

- Open issues: 22
- Branch state during audit: `main` aligned with `origin/main`
- Latest local commit: `de2378c fix(security): harden credential proxy and VM provisioning`
- Recent high-signal work: #596 rate limiting, #597 first-boot restart handling, #518 credential leakage harness, #507 transport/security evidence refresh
- Live validation addendum: QEMU/libvirt host validation was run on 2026-07-02 after restarting the dev management server under `agentic-mgmt-live-validation.service`.
- Correction addendum: live validation exposed a dev-server `RLIMIT_NOFILE` crash path during repeated libvirt inventory calls; `management/src/main.rs` now raises the process soft `RLIMIT_NOFILE` to the hard limit at startup.

## Recommended Closure Review

| Issue | Recommendation | Rationale |
| --- | --- | --- |
| #596 | Close after maintainer review | Acceptance criteria are covered by `de2378c`: per-lease/session rate limiting, HTTP 429 + `Retry-After`, expired/revoked denial before accounting, counter isolation tests, and docs updates. |
| #595 | Close-review candidate | Prior AL cycle reports canonical `cid=instance_id` persistence, legacy tolerance, malformed-row skip, cleanup updates, and regression tests. It remains open only because no live VM restart repro was rerun in that cycle. |

## Keep Open: Partial Evidence Or Live Validation Required

| Issue | Current status | Next evidence needed |
| --- | --- | --- |
| #597 | Partially live-validated after `de2378c` | Direct QEMU provision `audit597-live-20260702` with vsock CID `6` stayed `running` through the first-boot poweroff observation window and exited provisioning successfully. It did not reproduce the first-boot shutoff, so the restart branch was not exercised live. SSH did not become ready within 300s, and this run did not prove agent enrollment or PTY attach for the new VM. |
| #518 | Harness landed in `de2378c`, but direct upstream bypass prevention is not proven | Add network-policy/egress-profile evidence or explicitly constrain public claims for profiles without bypass denial. |
| #507 | Report exists and was refreshed in `de2378c`; several ACs remain blocked | AC-2 now has partial live QEMU/vsock power-state evidence, but still needs a VM that reaches agent READY plus no-agent-plane-TCP evidence. AC-1 live container UDS READY + socket/capture, AC-5 fleet key custody, AC-6 renewal with live PTY continuity, and AC-7 unknown valid-cert mTLS rejection also remain open. |
| #503 | Epic should remain open | Market-readiness gates still depend on #507, #518, #596 closure review, private-material backends, auth-state handoff, and fleet hardening. |

## High-Priority Actionable Backlog

| Issue | Classification | Recommended next action |
| --- | --- | --- |
| #499 | Security/platform auth-state handoff | Requires explicit operator-approved design for provider CLI auth state. Prior threat preflight flagged credential handling. |
| #517 | Credential proxy protocol adapters | Now less blocked because #516/#596 backend work exists; begin with support matrix or adapter design, then implement Git/S3/registry/database slices with credential-safety gates. |
| #494 | Linux keyring local CA backend | Implement only with explicit private-material authorization; needs keyring/missing/headless behavior and runbook coverage. |
| #495 | macOS Keychain local CA backend | Pair with Apple workstation support; requires locked/non-interactive Keychain behavior and no silent migration. |
| #411 | Fleet CA lifecycle hardening | Umbrella for OpenBao/step-ca/SPIRE-style issuance, short-lived leaves, renewal, hot reload, and expiry gates. Still gated by private-material/fleet readiness. |
| #404 | Transport-security epic | Keep open until #411 and final release-specific evidence close the remaining internal control-plane acceptance gaps. |

## Platform And Runtime Backlog

| Issue | Classification | Dependency/readiness |
| --- | --- | --- |
| #438 | macOS host support epic | Keep open; first concrete step is #488 Apple `container` spike. |
| #488 | Apple `container` feasibility spike | Needs Apple Silicon macOS 26 host with Apple `container` installed. |
| #489 | Apple `container` provider implementation | Blocked by #488; should not start until spike proves provider contract viability. |
| #462 | Native installer/release flows | Release-engineering epic; depends on macOS provider/build disposition and packaging matrix decisions. |
| #114 | Platform-agnostic VM provisioning epic | Keep open as umbrella for Alpine/Proxmox phases. |
| #118 | Alpine agentic-dev profile | Depends on base Alpine cloud-init/profile support. |
| #119 | Backend abstraction | Proxmox/libvirt abstraction remains larger platform work; useful prerequisite for #120. |
| #120 | Deploy/lifecycle script updates | Depends on #118/#119 and musl/build-profile maturity. |

## Supply-Chain / Settlement Backlog

| Issue | Classification | Notes |
| --- | --- | --- |
| #586 | Phase 1 signed completion artifact | Additive settlement primitive using existing result hashing/JWS/AgentCard signing; actionable when settlement work is prioritized. |
| #587 | Phase 2 signed metered duration claims | Explicitly not day-1; should follow direct settlement artifact work. |

## Suggested Execution Order

1. Closure review: #596, then #595.
2. Live runtime evidence: finish #597 with SSH/agent enrollment/PTY evidence, then update #507 AC-2.
3. Credential evidence: finish #518 direct bypass stance, then update #503/#507/security docs.
4. Transport/fleet hardening: #411 under #404, with #494/#495 as workstation private-material backends.
5. Provider auth handoff: #499, with a design-first pass to avoid unsafe credential propagation.
6. Credential adapters: #517 after the current proxy backend and leakage evidence are accepted.
7. Apple/macOS lane: #488, #489, #438, then #462 packaging decisions.
8. VM platform lane: #119, #118, #120 under #114.
9. Settlement lane: #586 before #587.

## Suggested Next Commands

```text
review and close #596 if de2378c satisfies the acceptance criteria
review #595 for closure or request one live restart repro
address issue #597 with live Cockpit/qemu SSH, agent enrollment, and PTY evidence
address issue #518 direct bypass evidence or limitation wording
```

## Live Validation Addendum - 2026-07-02

Commands/evidence captured:

- Rebuilt management release binary: `cargo build --manifest-path management/Cargo.toml --release --bin agentic-mgmt`.
- Restarted management under user systemd as `agentic-mgmt-live-validation.service` with the prior dev environment.
- Confirmed `/healthz/libvirt` returned `{"status":"healthy","libvirt":"alive"}` after libvirt and management restart.
- Ran direct provision:
  `AGENTIC_GRPC_VSOCK_PORT=8120 AGENTIC_VM_FIRST_BOOT_RESTART_SECONDS=90 AGENTIC_VM_SSH_WAIT_SECONDS=300 images/qemu/provision-vm.sh --base ubuntu-24.04 --profile basic --cpus 2 --memory 4G --disk 12G --ssh-key /home/roctinam/.codex/roles-runtime/full/.ssh/agentic_ed25519.pub --instance-id 835fac65-2e6c-45dc-951a-5ec11a6ae809 --management host.internal:8120 --start --wait audit597-live-20260702`
- Provision output showed guest vsock target host CID `2`, port `8120`, VM peer CID `6`, successful base-image hash verification, VM define/start success, first-boot poweroff check from 21:34:48 to 21:36:21, and final `provision_exit=0`.
- `virsh domstate audit597-live-20260702` returned `running` during and after the first-boot observation window.
- `virsh domifaddr audit597-live-20260702` did not report a guest address and SSH did not respond within 300 seconds.
- Cleanup via `scripts/destroy-vm.sh audit597-live-20260702 --force` stopped and undefined the VM, removed storage, removed DHCP, removed CID allocation, and removed the ephemeral SSH key. A stale test IP registry row was removed manually after cleanup.
- Existing Cockpit VM `cockpit-mqzoikm4wifv` was verified via `/api/v1/vms/cockpit-mqzoikm4wifv` as `state=running` with `agent.connected=true` at `192.168.122.248`.
- Session creation against `cockpit-mqzoikm4wifv` returned `pty-ws.v1` attach metadata, but test commands exited `127`, so this pass does not count as PTY output validation.

## Correction Addendum - libvirt inventory stability

During validation, the transient management process crashed with:

```text
libvirt: XML-RPC error : Failed to create socket: Too many open files
GLib-ERROR **: Creating pipes for GWakeup: Too many open files
```

The transient user unit had `LimitNOFILESoft=1024` and `LimitNOFILE=1048576`. Packaged systemd units already request the high limit, but dev/transient launches could still inherit a low soft cap. The correction raises the soft `RLIMIT_NOFILE` to the available hard limit during management startup.

Verification:

- `cargo fmt --manifest-path management/Cargo.toml` passed.
- `cargo test --manifest-path management/Cargo.toml libvirt -- --nocapture` passed.
- Low-limit smoke run: started `target/debug/agentic-mgmt` with `ulimit -Sn 1024`.
- Startup log recorded `raised RLIMIT_NOFILE soft limit previous_soft=1024 soft=1048576 hard=1048576`.
- `/proc/<pid>/limits` showed `Max open files 1048576 1048576`.
- 160 concurrent requests across `/api/v1/vms?prefix=*` and `/healthz/libvirt` all returned HTTP 200, and the process remained active.
