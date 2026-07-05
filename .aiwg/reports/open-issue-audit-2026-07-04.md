# Open Issue Audit - 2026-07-04

## Snapshot

- Source of truth: Gitea open issues, audited 2026-07-04.
- Open issues: 25.
- Branch: `main` aligned with `origin/main`.
- Latest audited commit: `2ce5072 docs(monitoring): refresh issue 61 artifacts`.
- Latest release tag observed locally: `v2026.7.1`.
- Change since the 2026-07-03 audit: new content/API issues #599, #600, and #601 are open; monitoring issue #61 artifacts were refreshed and committed.

## Closure Review Candidates

| Issue | Current status | Recommended disposition | Evidence / remaining check |
| --- | --- | --- | --- |
| #596 | Per-active-lease/session credential proxy rate limiting was implemented with 429/Retry-After semantics and tests. | Close-review candidate. | Its local acceptance appears satisfied. Broader upstream bypass/leakage assurance remains tracked by #518 and should not block #596 unless maintainers want a combined security closure. |
| #595 | Canonical `cid=instance_id` vsock registry writer and tolerant reader landed; cleanup/tests were updated. Cockpit still carries a defensive pre-start healer for reversed/stale registry entries. | Conditional close-review candidate. | Close from sandbox side if released behavior is accepted. Keep open only if removal of the Cockpit compatibility healer or live restart validation is required before closure. |

## Keep Open: Evidence-Gated

| Issue | Current status | Next evidence needed |
| --- | --- | --- |
| #597 | Deterministic restart handling is released, and a direct QEMU run on commit `7288721` stayed running through the observation window. SSH, agent enrollment, and PTY evidence were not captured. | Run live VM-tier provisioning on v2026.7.1+ and capture provision -> SSH -> agent enrollment/READY -> PTY output. |
| #518 | Credential leakage harness work exists, but direct upstream bypass prevention is still not proven for all claimed profiles. | Add network-policy/egress-profile evidence that direct upstream bypass is denied, or constrain public claims for profiles that do not enforce bypass denial. |
| #507 | Release-specific transport report exists, but several acceptance criteria are still blocked or only partially evidenced. | AC-1 live container UDS READY + socket/capture; AC-2 live VM vsock READY + no agent-plane TCP evidence; AC-5 fleet key custody; AC-6 renewal with live PTY continuity; AC-7 unknown valid-cert mTLS rejection. |
| #503 | Epic remains the market-readiness umbrella. | Keep open until #507, #518, private-material backends, auth-state handoff, adapter coverage, and fleet hardening have closure evidence. |

## New / Unstarted Issues

| Issue | Status | Recommended next action |
| --- | --- | --- |
| #599 | June 2026 report exists, but the requested hero asset, frontmatter hero URL, in-body image, and transparency note are not present. | Fetch the approved PNG from the strategy repo, save it to `docs/assets/blog/2026-06-agentic-sandbox.png`, update the June report frontmatter/body, and rebuild the docbase. |
| #600 | No `AgentOutputEvent`, `chat_source`, or structured agent-output stream implementation was found. Existing docs and code cover PTY extensions and Claude stream-json primitives, but not the Cockpit Chat projection contract. | Start with a small contract/spec slice for stream identity, capability advertisement, and subscription semantics; then implement Claude Code `stream-json` projection while preserving raw PTY as authoritative. |
| #601 | Only the June 2026 report is present in `docs/blog/`; January through May reports are not present and not registered in the blog manifest. | Run the monthly-update backfill for 2026-01 through 2026-05, register the pages in `docs/blog/_manifest.json`, and keep the prose public-facing without internal issue counts or volatile totals. |

## Active Backlog By Theme

### Security / Transport

| Issue | Audit status |
| --- | --- |
| #404 | Keep open as the transport-security epic. Closure depends on the remaining #507 evidence and downstream hardening work. |
| #411 | Keep open for Phase 4 fleet hardening: revocation drills, rotation automation, inventory drift checks, expiry alerting, signed fleet manifest, and runbook evidence. |
| #494 | Keep open for Linux keyring backend for local CA private material. |
| #495 | Keep open for macOS Keychain backend for local CA private material. |
| #499 | Keep open for host-runtime auth-state handoff; still needs a security design and implementation path that avoids ad hoc credential leakage. |
| #517 | Keep open for credential proxy adapters and support matrix across Git, S3, registry, and database flows. |

### Runtime / Platform

| Issue | Audit status |
| --- | --- |
| #438 | Keep open for macOS host support and aarch64-apple-darwin management build path. |
| #488 | Keep open for Apple container provider spike. |
| #489 | Keep open for Apple container provider implementation. |
| #462 | Keep open for production native-installer build/release flows. The v2026.7.1 release path does not close `.deb`, `.rpm`, or `.dmg` installer acceptance. |
| #114 | Keep open for platform-agnostic VM provisioning with Alpine support. |
| #118 | Keep open for Alpine agentic-dev profile. |
| #119 | Keep open for libvirt/Proxmox backend abstraction. |
| #120 | Keep open for Alpine + Proxmox deploy and lifecycle script updates. |

### Settlement / Metering

| Issue | Audit status |
| --- | --- |
| #586 | Keep open for signed completion artifact emission at `mission.completed`. |
| #587 | Keep open for signed metered duration claims from lifecycle/Prometheus signals. |

## Recommended Execution Order

1. Close-review #596 and decide #595 disposition against Cockpit compatibility-healer removal requirements.
2. Land quick docsite/content wins #599 and #601.
3. Run live VM-tier validation for #597 and reuse the capture for #507 AC-2 where applicable.
4. Finish #518 with direct-bypass denial evidence or narrowed public claims.
5. Define and implement the #600 structured output stream contract for Cockpit Chat.
6. Continue the remaining #503 security market-readiness dependencies: #494, #495, #499, #517, #404, and #411.
