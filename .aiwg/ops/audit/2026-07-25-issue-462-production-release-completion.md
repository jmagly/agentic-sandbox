# Issue #462 production release completion audit

## Session

- Date: 2026-07-25
- Repository: `roctinam/agentic-sandbox`
- Private repository: `jmagly/agentic-sandbox-enterprise`
- Public starting commit: `4df06e0c235686e75d94fbc75c236dcc3e337902`
- Private starting commit: `af56e5af71b1285d9030108271540d4d06dd9b79`
- Operator authorization: public #677 credential-free controls, private
  enterprise #1/#2, and a separately witnessed real Developer ID/notarization
  ceremony using operator-controlled credentials.

## Safety invariants

- Never print, export, copy, or retain certificate private keys, notarization
  credentials, Vault values, Keychain exports, provider credentials, or
  customer/license signing secrets.
- Treat certificate identity strings, Team ID, notary profile name, approval
  reference, commits, tags, and artifact digests as non-secret identifiers.
- Retrieve the mutsu SSH key only through the existing Vault-backed CI route
  and remove it on every exit path.
- Run the real ceremony only from an exact approved source state and preview
  manifest, after all synthetic and private compatibility gates pass.
- Do not publish or promote an artifact if any required gate is absent,
  ambiguous, skipped, or failed.

## Initial state and commands

| Time (America/New_York) | Context | Command/action | Result |
|---|---|---|---|
| 2026-07-25 | public | `git status --short --branch`; `git rev-parse HEAD` | Clean `main`; starting commit recorded above. |
| 2026-07-25 | private | `git status --short --branch`; `git rev-parse HEAD` | Clean `main`; starting commit recorded above. |
| 2026-07-25 | Gitea | Read #462 and #677 | Both open; #677 has two credential-free criteria proven and six production criteria open. |
| 2026-07-25 | Gitea | List repository secret names and variables | Only secret names were read. Vault CI credentials and the mutsu key route exist; Apple identity/profile variables are not configured. |
| 2026-07-25 | local → mutsu | Batch-mode SSH identity inventory | Refused with public-key authentication failure; no credential fallback or secret search attempted. |
| 2026-07-25 | private remote | Inspect configured remote and private issue authority | `jmagly/agentic-sandbox-enterprise` is authoritative on private GitHub; the previously referenced Gitea path does not exist. |
| 2026-07-25 | GitHub | Read and comment on `jmagly/agentic-sandbox-enterprise#1/#2` | Both issues open; exact authorization and synthetic/credential-free boundaries recorded. |
| 2026-07-25 | public | `tests/package/test-package-macos.sh` | Fourteen synthetic package, preflight, entitlement, evidence-schema, fail-closed, and quarantine assertions passed. No real identity or credential was used. |
| 2026-07-25 | private | Independent review of enterprise issue #1 bootstrap | Common boundary tests passed, but the named OpenBao/Vault, step-ca, and SPIRE mappings were absent; the public release tag remained unpinned; and provider panic output was not contained. Issue #1 remains open and remediation was assigned. |
| 2026-07-25 | GitHub | Read private repository Actions runner inventory | No repository runner is registered; changing Titan's host registration was rejected as unnecessary host scope. |
| 2026-07-25 | Gitea | Create private `roctinam/agentic-sandbox-enterprise-ci` | Created an empty private Titan execution mirror. `jmagly` was not a creatable Gitea owner, so canonical private source and issues remain `jmagly/agentic-sandbox-enterprise` on GitHub. |
| 2026-07-25 | public | `make test-scripts` | Full script regression suite passed, including synthetic identity inventory/preflight, closed evidence, immutable handoff, mandatory tag-promotion contract, recovery rehearsal, runtime/container, VM, GPU/VFIO guard, and package tests. |
| 2026-07-25 | public | Prepare `v2026.7.13` candidate metadata | Updated all three Rust crate versions/locks, changelog, and release notes. No tag was created. |
| 2026-07-25 | private | Enterprise #1 implementation | Added bounded OpenBao/Vault and step-ca mappings, SPIRE's semantically valid health/bundle subset, named synthetic process harnesses, opaque credential references, and panic/error sanitation. Exact public checker accepted both signing-capable named harnesses. |
| 2026-07-25 | private | Independent enterprise #2 review | Found canonical-GitHub bypass, unverified mirror/canonical commit equality, missing publication/clean-install contract, and ambient Titan runtime gaps. Issue #2 remains open and remediation is in progress. |
| 2026-07-25 | public/private integration | Independent CA adapter review and Gitea issue #681 | Found that remote CSRs could reach a provider before exact SPIFFE SAN/CN validation. Filed `roctinam/agentic-sandbox#681`; public pre-issuance validation and private defense-in-depth remediation are in progress. |
| 2026-07-25 | public | Independent #677 review | Synthetic controls passed, but review found SSH TOFU, cross-run preview reproducibility, mutable publication, pre-approval notary access, and relational evidence gaps. No ceremony/tag was authorized to proceed; remediation is in progress. |
| 2026-07-25 | public/Gitea | Seed non-secret mutsu host-key variables | Recorded the independently scanned ed25519 key and fingerprint as repository variables. They are not credentials. The fingerprint still requires confirmation through the authenticated mutsu/operator path before any witnessed ceremony. |

## Implementation and verification log

This section is updated as the public controls, private adapters, compatibility
matrix, and operator ceremony progress. Sanitized CI run IDs, source commits,
artifact digests, and verification results are retained here; secret-bearing
outputs are never copied into this document.

Current decision: **NO-GO**. Public and private implementations are under final
independent review, private CI remediation is still in progress, and no
operator credential preflight or real notarization submission has run.

## Files modified

- `.aiwg/ops/audit/2026-07-25-issue-462-production-release-completion.md`
- `.aiwg/deployment/deployment-readiness-issue-462.md`

## Backups

No backups created. All repository changes are version-controlled.
