# Issue #462 Apple release readiness

## Purpose

Gate the first production Apple Silicon Agentic Sandbox package from an exact,
credential-free preview through Developer ID signing, Apple notarization,
stapling, public verification, and Gitea-authoritative publication. This
procedure does not deploy a running service. Its production change is the
publication of immutable installable artifacts.

Current decision: **NO-GO** until every gate below is independently verified.

## System topology

- Public source: `roctinam/agentic-sandbox`
- Public mirror: `jmagly/agentic-sandbox`
- Enterprise source: `jmagly/agentic-sandbox-enterprise`
- Enterprise Titan execution mirror:
  `roctinam/agentic-sandbox-enterprise-ci` (private, non-authoritative)
- Linux CI/orchestrator: Titan, serialized shared runner
- Apple builder and ceremony host: mutsu, Apple Silicon
- Unsupported builder: teroknor
- Candidate version: `v2026.7.14` (`v2026.7.13` was prepared but deliberately
  skipped before tagging)
- Regression threshold: zero
- Promotion strategy: preview → verified signed candidate → tag publication
- Rollback strategy: quarantine before publication; superseding patch release
  after publication

## Procedure

1. Land public #677 credential-free controls and pass local, Titan, and exact
   mutsu synthetic validation.
2. Land private enterprise #1 and #2 and pass their exact-public-pin matrix.
3. Produce the unsigned preview from the exact candidate commit and retain its
   payload-manifest digest.
4. Run the manual mutsu preflight. Require exactly one matching Developer ID
   Application identity, exactly one matching Developer ID Installer identity,
   the expected Team ID, and an existing notary profile.
5. Bind the non-secret approval reference, source commit, release tag, Team ID,
   preview-manifest digest, and expected entitlement policy into the ceremony
   request.
6. Run production packaging on mutsu. Stop on any signing, notarization,
   stapling, Gatekeeper, checksum, evidence-schema, or digest failure.
7. Independently verify the final package and disk image using only public
   signature, ticket, package, and digest outputs.
8. Attach the verified Apple artifact set to the Gitea release and mirror the
   same bytes to GitHub. Verify both surfaces by downloading and hashing them.
9. Promote the Apple lane to a required, non-skippable production-tag
   prerequisite only after the real proof succeeds.

## Verification

Expected successful evidence:

```text
source_commit=<40 lowercase hexadecimal characters>
preview_manifest_sha256=<64 lowercase hexadecimal characters>
application_identity_matches=1
installer_identity_matches=1
team_id_matches=true
notary_profile_available=true
pkg_signature=valid
pkg_notarization=accepted
pkg_staple=valid
pkg_gatekeeper=accepted
dmg_signature=valid
dmg_notarization=accepted
dmg_staple=valid
dmg_gatekeeper=accepted
release_evidence_schema=valid
gitea_asset_digests=match
github_asset_digests=match
```

Any missing, skipped, ambiguous, or failed result keeps the decision at
**NO-GO**.

## Troubleshooting

- **Identity count is zero**: stop. The operator must provision the expected
  certificate outside CI; do not import or search for private material.
- **Identity count exceeds one**: stop. Do not guess which identity to use.
- **Notary profile unavailable**: stop. The operator must repair the Keychain
  profile outside logs and automation.
- **Payload manifest differs**: quarantine the candidate and rebuild from the
  approved exact source commit.
- **Apple rejects notarization**: retain only the public request identifier and
  sanitized status; quarantine artifacts and investigate the public notary log.
- **Post-publication digest mismatch**: unpublish the affected release surface,
  preserve the tag, and cut a superseding patch release after root-cause
  analysis.

## House rules for agents

- Do compare every actual output with the expected gate before continuing.
- Do retain only public identifiers, digests, result codes, and sanitized logs.
- Do stop on ambiguity, unexpected entitlement, or missing evidence.
- Do not print environment variables, Keychain contents, Vault responses, or
  notarization credential material.
- Do not import, export, rotate, delete, or copy a real identity.
- Do not re-run a non-idempotent notary submission after an uncertain result
  without first checking the public submission status.
- Do not publish a partial Apple artifact set.

## What not to fix

- The direct local SSH key for mutsu is intentionally absent. Use the
  Vault-backed manual workflow.
- The Apple lane remains disabled on ordinary pushes and pull requests.
- Teroknor remains unsupported for this repository.
- Apple `container` support remains deferred under #488/#489 and does not block
  the host plus Docker Desktop package.

## Audit trail

- Created: 2026-07-25
- Last verified: 2026-07-28; pending a new exact-commit live ceremony for
  `v2026.7.14`
- Author: Codex under operator authorization
- Applicable hosts: Titan (orchestration only), mutsu (Apple build/ceremony)
- Detailed command and evidence log:
  `.aiwg/ops/audit/2026-07-25-issue-462-production-release-completion.md`
