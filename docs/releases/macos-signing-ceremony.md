# macOS signing and notarization ceremony

This runbook separates credential-free preparation, public identity inventory,
the witnessed Developer ID/notary operation, and tag publication. It never
asks automation to retrieve, import, export, rotate, or delete an Apple private
key or notarization credential.

## Roles and non-secret bindings

Two people participate in a production ceremony:

- the release operator controls the Developer ID identities and notary profile
  already provisioned on mutsu;
- the witness approves the exact source commit, anticipated release tag,
  preview package digest, preview payload-manifest digest, and approval
  reference.

The Team ID, exact Application/Installer selector names, notary profile name,
operator approval reference, source commit, tag, and SHA-256 digests are public
metadata. Passwords, private keys, App Store Connect key contents, Keychain
exports, tokens, and notary history are never release metadata.

## Manual-only workflow

Use Gitea's **Manual macOS Release Ceremony** workflow. It has only a
`workflow_dispatch` trigger, runs SSH orchestration on Titan, fetches the mutsu
SSH key through `ci/vault-fetch.sh`, pins mutsu's host key, transfers an exact
commit archive, and removes the archive, dispatch config, SSH key, and
per-run remote workspace on every exit path.

The anticipated tag does not need to exist during inventory, preparation, or
ceremony. The workflow verifies that it is vCalVer and that its version matches
all three Cargo manifests in the exact source commit. Creating the tag before
the ceremony would trigger release CI too early and is prohibited.

### 1. Sanitized inventory

Dispatch `operation=inventory` with the exact source commit and anticipated
tag. Team ID, identity selector, and notary profile inputs are intentionally
empty.

The result contains only:

- public Developer ID Application selector names, embedded Team IDs, and exact
  duplicate counts;
- public Developer ID Installer selector names, embedded Team IDs, and exact
  duplicate counts.

`security find-identity -v -p codesigning` and `-p basic` are filtered in
memory. Inventory never invokes `notarytool`. Certificate hashes, unrelated
Keychain entries, notary history, private-key material, and credential values
never reach stdout or artifacts. The exact requested profile is tested only by
the witnessed ceremony preflight.

### 2. Credential-free prepare

Dispatch `operation=prepare` for the same commit and anticipated tag. This
builds all four Apple Silicon binaries once, creates the unsigned preview
package, and stores an immutable approval bundle under the source commit, tag,
preview-manifest digest, and preview-package digest. The bundle retains the
exact source archive, four payload binaries, preview package, payload manifest,
and closed preparation evidence. It also returns the preview package, manifest,
and sanitized evidence for witness review. It does not query a signing identity
or notary profile.

The witness records:

- `source_commit`;
- `release_tag`;
- `preview_package_sha256`;
- `preview_manifest_sha256`;
- a non-secret approval reference.

### 3. Witnessed ceremony

Dispatch `operation=ceremony` with those exact bindings plus the exact public
Team ID, Application selector, Installer selector, and profile name selected
from inventory. Do not place a password, key content, token, or other secret in
an input.

The workflow resolves the exact retained approval bundle by both independently
approved preview digests. It re-hashes the source archive, preview package,
manifest, and every binary, and signs from those exact retained payload bytes;
it never rebuilds and assumes `pkgbuild` output reproducibility. Preflight then
requires exactly one Application identity under the
`codesigning` policy, exactly one Installer identity under the `basic` policy,
both selectors ending in the expected Team ID, and the requested notary profile
to be available. Inventory and notary responses are discarded.

The packager then:

1. signs all four Mach-O payloads with hardened runtime and the closed expected
   entitlement plist;
2. proves the extracted entitlement set is exactly equal—missing or additional
   entitlements fail;
3. signs, notarizes, staples, and publicly verifies the package;
4. verifies package identifier and version receipt metadata;
5. signs, notarizes, staples, and Gatekeeper-assesses the DMG;
6. validates the closed release-evidence schema and every approval/source/tag/
   preview/artifact binding.

On success, the exact verified files are copied to:

```text
/Volumes/build/agentic-sandbox/macos-ceremony/approved/
  <anticipated-tag>/<source-commit>/<release-evidence-sha256>/
```

That handoff is immutable and contains a closed file set. Its `handoff.json`
provides the evidence digest the witness must approve independently for tag
promotion.

### 4. Tag and promotion

Only after reviewing the sanitized evidence:

1. set non-secret repository variable `MACOS_APPROVED_RELEASE_TAG` to the
   anticipated tag;
2. set `MACOS_APPROVED_RELEASE_EVIDENCE_SHA256` to the exact witnessed
   `handoff.json` evidence digest;
3. create the anticipated tag on the exact evidence-bound source commit;
4. push the tag.

Tag CI does not sign or access Apple credentials. Its
`release-macos-promote` job fetches the immutable handoff through the same
Vault-backed SSH route by the independently approved evidence digest, requires
tag-to-commit equality, validates the handoff and release-evidence schemas,
re-hashes the package, DMG, manifest, sidecars, and checksum manifest, then
uploads those exact bytes. Existing Gitea or GitHub assets are downloaded and
must have the same digest; release automation never replaces them. The GitHub
tag must peel to the exact Gitea source commit before any mirror upload. Gitea
release attachment and the GitHub release mirror cannot run if the Apple job is
skipped or fails. The tag itself is the promotion action.

## Mutsu host trust and storage limitation

`MUTSU_SSH_HOST_KEY` must be an out-of-band reviewed, single-line
`10.0.42.41 ssh-ed25519 <base64>` known-hosts record.
`MUTSU_SSH_HOST_KEY_FINGERPRINT` must be its independently reviewed
`SHA256:...` fingerprint. Both are non-secret repository variables. Ceremony
and promotion validate their exact formats and equality with `ssh-keygen`; they
never use `ssh-keyscan` or trust a key learned from the live connection.

The `0444`/`0555` approval and handoff permissions prevent accidental writes,
but mutsu storage is not hardware WORM: the owning host account can still
change permissions. Consequently, permissions are not the trust anchor.
Independent approval digests, exact path binding, and complete re-hashing at
ceremony and tag promotion detect replacement. A future read-only or
content-addressed artifact store can strengthen this operational boundary.

## Failure and recovery

Never override a failed gate.

| Event | Required response |
|---|---|
| Missing or duplicate identity | Abort. Correct operator-controlled Keychain state outside automation, then rerun inventory. |
| Team ID, source, tag, preview, or entitlement mismatch | Abort. Do not sign or publish. Start a new reviewed preparation. |
| Signing/notary/stapling/Gatekeeper failure | Partial output is moved under the run's `quarantine/`; it is never staged as approved. |
| Candidate must be replaced | Keep the quarantined evidence and use a new vCalVer tag. Never relabel old bytes. |
| Developer ID certificate revoked | Block tag/publication, quarantine unpublished bytes, record the public certificate reference, and have the operator rotate the identity. Never delete or export it from CI. |
| Notary profile retired or suspected | Block future submissions, record the public profile name, and have the operator retire/reprovision it outside CI. Never enumerate or delete Keychain items from automation. |
| Bad artifact already published | Stop mirrors, publish a superseding release, document affected digests, and follow the incident/revocation process. Do not replace an asset under the same digest or tag. |

Run the credential-free rehearsal at any time:

```bash
scripts/rehearse-macos-release-recovery.sh \
  --workspace /tmp/agentic-macos-recovery \
  --output /tmp/agentic-macos-recovery.json \
  --source-commit 3333333333333333333333333333333333333333 \
  --release-tag v2026.7.13 \
  --superseding-tag v2026.7.14 \
  --operator-approval-ref synthetic-issue-677-witness \
  --synthetic-certificate-ref synthetic-developer-id-certificate \
  --synthetic-notary-profile synthetic-retired-profile
```

The rehearsal operates only on generated fixture files and proves abort,
quarantine, supersede, certificate-revocation response, and profile-retirement
state. It performs no Keychain or Apple-service operation.
