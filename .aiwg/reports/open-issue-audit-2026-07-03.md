# Open Issue Audit - 2026-07-03

Scope: Gitea `roctinam/agentic-sandbox` tracker, all open issues visible during the 2026-07-03 audit, with local branch state at `53cf17c` / `v2026.7.1`.

Mode: backlog audit and evidence triage. No issue state was changed by this audit.

## Snapshot

- Open issues: 22
- Branch state during audit: `main` aligned with `origin/main`
- Latest local commit: `53cf17c ci(e2e): bound libvirt probes`
- Latest local tag: `v2026.7.1`
- Recent high-signal work: #596 rate limiting, #597 first-boot restart handling, #518 credential leakage harness, #507 transport/security evidence refresh, libvirt probe/resource-limit hardening.

## Recommended Closure Review

| Issue | Recommendation | Rationale |
| --- | --- | --- |
| #596 | Close after maintainer review | The issue's own acceptance criteria are covered by the rate-limit implementation shipped in `de2378c` and released in `v2026.7.1`: per-lease/session accounting, HTTP 429 plus `Retry-After`, expired/revoked denial before accounting, isolated counters, cleanup, tests, and docs. Broader leakage/bypass confidence remains tracked by #518. |
| #595 | Close-review candidate | The canonical `cid=instance_id` writer fix, tolerant reader, cleanup mapping, destroy/reaper updates, and parser/startup regressions are covered by deterministic tests and released. A live restart repro was not rerun, but the reported crash-loop class is addressed. |

## Keep Open: Evidence Gaps

| Issue | Current status | Next evidence needed |
| --- | --- | --- |
| #597 | Deterministic restart handling is implemented and partial QEMU validation exists, but the live closeout is incomplete. | Run live Cockpit/QEMU provisioning on the libvirt host and capture either the first-boot shutoff/restart branch or a normal first boot that reaches SSH, agent enrollment/READY, and live PTY attach. |
| #518 | Leakage harness landed and documents unsupported bypass-proof profiles, but direct upstream bypass prevention is not proven. | Add network-policy/egress-profile evidence for direct upstream denial, or explicitly accept the bounded public claim that bypass denial is unsupported for profiles without that control. |
| #507 | Release-specific report exists; multiple acceptance criteria remain blocked. | AC-1 live container UDS READY plus socket/capture, AC-2 live VM vsock READY plus no agent-plane TCP evidence, AC-5 fleet key custody, AC-6 renewal with live PTY continuity, and AC-7 valid-cert unknown-identity mTLS rejection. |
| #503 | Epic should remain open. | Market-readiness gates still depend on #507, #518 claim/evidence closure, #596/#595 closure review, private-material backends, auth-state handoff, and fleet hardening. |

## High-Priority Actionable Backlog

| Issue | Classification | Recommended next action |
| --- | --- | --- |
| #499 | Security/platform auth-state handoff | Design an explicit provider CLI auth handoff mechanism for managed sessions; do not silently propagate credential state. |
| #517 | Credential proxy protocol adapters | Start with a support matrix, then implement Git HTTPS and one S3 or registry mediated flow with denial/redaction tests. Database support can be narrowly implemented or explicitly deferred. |
| #494 | Linux keyring local CA backend | Implement an explicit Linux keyring backend with headless/missing/locked keyring behavior, no silent migration, and issuance-continuity tests. |
| #495 | macOS Keychain local CA backend | Implement only with explicit Keychain backend selection and documented locked/non-interactive behavior. Pair with Apple workstation validation where practical. |
| #411 | Fleet CA lifecycle hardening | Add external CA issuance, short-lived leaves, renewal cadence, hot reload, expiry gates, and AC-6 live PTY continuity evidence. |
| #404 | Transport-security epic | Keep open until #411 and the remaining #507 acceptance evidence close the internal control-plane security suite. |

## Platform And Runtime Backlog

| Issue | Classification | Dependency/readiness |
| --- | --- | --- |
| #438 | macOS host support epic | Keep open; depends on #488 spike and #489 provider implementation. |
| #488 | Apple `container` feasibility spike | Needs Apple Silicon macOS 26 with Apple `container` installed, exact command transcript, runtime connectivity, task/log/session observation, cleanup, and proceed/defer recommendation. |
| #489 | Apple `container` provider implementation | Blocked by #488. Do not start provider implementation until the spike proves the runtime contract. |
| #462 | Native installer/release flows | Release-engineering epic remains open; `v2026.7.1` proves the current release path, not `.deb`/`.rpm`/`.dmg` production installers. |
| #114 | Platform-agnostic VM provisioning epic | Keep open as umbrella for Alpine/Proxmox work. |
| #118 | Alpine agentic-dev profile | Basic Alpine generators exist, but the full `agentic-dev` package/profile parity remains unimplemented. |
| #119 | Backend abstraction | Proxmox backend remains a stub; libvirt is still the real backend. |
| #120 | Deploy/lifecycle script updates | Depends on #118/#119 and must remove hardcoded Ubuntu/systemd/libvirt assumptions from lifecycle scripts. |

## Supply-Chain / Settlement Backlog

| Issue | Classification | Notes |
| --- | --- | --- |
| #586 | Phase 1 signed completion artifact | Additive settlement primitive: emit an AgentCard-key-signed completion artifact with stable `result_hash`. Actionable when settlement work is prioritized. |
| #587 | Phase 2 signed metered duration claims | Follows #586/direct settlement; explicitly not a day-1 blocker. |

## Suggested Execution Order

1. Close-review pass: #596, then #595.
2. Live runtime evidence: finish #597, then update #507 AC-2.
3. Credential evidence: resolve #518's bypass-proof stance, then update #503/#507/security docs.
4. Transport/fleet hardening: #411 under #404, with #494/#495 as local private-material backends.
5. Auth-state handoff: #499 design and implementation.
6. Credential adapters: #517.
7. Apple/macOS lane: #488, #489, #438, then #462 packaging decisions.
8. VM platform lane: #119, #118, #120 under #114.
9. Settlement lane: #586 before #587.

## Suggested Next Commands

```text
review and close #596 if v2026.7.1 satisfies the issue acceptance criteria
review #595 for closure or request one live restart repro
finish #597 with live Cockpit/QEMU SSH, agent READY, and PTY evidence
finish #518 by adding egress-policy evidence or accepting bounded limitation wording
```
