# Doc Sync Audit - 2026-07-02 Release

- Direction: `code-to-docs`
- Scope: release `v2026.7.0`
- Since: `v2026.6.36`
- Trigger: release gate `doc-sync-code-to-docs`

## Auditors

| Auditor | Source reviewed | Result |
| --- | --- | --- |
| `release-delta` | `git log v2026.6.36..HEAD` and changed files | Release notes and CHANGELOG now cover blog/docs additions, credential proxy hardening, QEMU provisioning restart handling, live issue evidence, and management descriptor-limit resilience. |
| `credential-proxy-hardening` | `management/src/http/credential_proxy.rs`, `management/src/credentials.rs`, `tests/security/run-credential-leakage-harness.sh`, security docs | Docs already describe proxy policy, rate limiting, redaction, and the continued need for egress controls. Release notes now include those operator-facing changes. |
| `qemu-first-boot-restart` | `images/qemu/provision-vm.sh`, `images/qemu/tests/test-runtime-boot-restart.sh`, open-issue audit | Release notes now describe first-boot shutoff observation/restart behavior without claiming full live enrollment/PTY proof. |
| `management-fd-limit` | `management/src/main.rs`, `.aiwg/reports/open-issue-audit-2026-07-02.md` | Release notes now describe startup soft `RLIMIT_NOFILE` raising and the low-limit smoke evidence. |
| `docs-release-surface` | `docs/blog/*`, `docs/config.json`, `docs/_manifest.json`, `docs/releases/verification.md` | Release manifest now includes `v2026.7.0`; CHANGELOG and release note link the docs/blog surface. |
| `open-issue-evidence` | `.aiwg/reports/open-issue-audit-2026-07-02.md` | Release claims are constrained to implemented and partially validated behavior. Remaining #503/#507/#518/#597 gaps stay explicit. |

## Findings

| Severity | Finding | Resolution |
| --- | --- | --- |
| High | Version manifests still identified the tree as `2026.6.36` even though the release cut is after the July month boundary. | Ran `scripts/bump-version.sh 2026.7.0 --date 2026-07-02`. |
| High | CHANGELOG and release announcement did not yet document the post-`v2026.6.36` runtime/security hardening. | Populated `CHANGELOG.md` and added `docs/releases/v2026.7.0.md`. |
| Medium | Release docs manifest did not expose the new release note. | Added `v2026.7.0` to `docs/releases/_manifest.json`. |
| Medium | Prior doc-sync marker still pointed at `release-v2026.6.35`. | Updated `.aiwg/.last-doc-sync`, `.aiwg/reports/doc-sync-last-run.json`, and this audit report. |

## Claims Deliberately Not Made

- #597 is not claimed fully closed. The direct QEMU validation proved the VM
  stayed running through the first-boot observation window, but did not exercise
  the restart branch live and did not prove new-VM SSH, agent enrollment, or PTY
  attach.
- #518 is not claimed to prove direct upstream bypass denial. The harness proves
  managed proxy/API redaction and denial paths; network egress profile evidence
  remains required for bypass-prevention claims.
- #507 and #503 remain open because fleet key custody, renewal/PTY continuity,
  unknown valid-cert rejection, private material backends, and market-readiness
  gates are not all complete.

## Files Updated By This Sync

- `CHANGELOG.md`
- `docs/releases/v2026.7.0.md`
- `docs/releases/_manifest.json`
- `.aiwg/.last-doc-sync`
- `.aiwg/reports/doc-sync-last-run.json`
- `.aiwg/reports/doc-sync-audit-2026-07-02-release.md`
