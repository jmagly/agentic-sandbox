# Release verification

This guide shows how to verify Agentic Sandbox release artifacts without
reading CI workflow internals.

Use it for tagged releases such as `v2026.6.28`. Replace `<version>` with the
tag, including the leading `v`, and replace `<owner>` with the public package
owner when it differs from the default `jmagly`.

## What verification proves

| Check | Proves | Does not prove |
| --- | --- | --- |
| SHA256 checksum | The downloaded file matches the release manifest. | Who produced the manifest. |
| Detached GPG signature | The artifact was signed by the holder of the expected GPG key. | Trust unless the expected key fingerprint is published and checked. |
| Container digest | The pulled image is the immutable image selected by the registry. | Who built or signed that image. |
| Cosign signature | The image was signed by the expected cosign key or keyless identity. | Trust unless the expected key or identity/issuer is published and checked. |
| SBOM | The package or image contents can be inspected. | Vulnerability-free status or provenance. |
| SLSA/in-toto provenance | Builder identity and build steps, when published. | Currently not claimed for Agentic Sandbox releases. |

## Credential Leakage Harness

Run the deterministic credential non-exposure harness before making release
claims about proxy-backed credential delivery:

```bash
tests/security/run-credential-leakage-harness.sh
```

The harness runs the HTTP credential proxy, credential metadata API, startup
profile, PTY transcript redaction, and QEMU loadout credential-reference tests.
It writes a markdown evidence report under `.aiwg/testing/` and fails if any
configured sentinel credential appears in captured command output.

Passing this harness supports a qualified claim that implemented metadata,
proxy, loadout, and transcript paths avoid returning or logging the sentinel
credential values covered by the tests. It does not prove direct upstream
bypass prevention for profiles without network-policy or egress-allowlist
verification; those profiles remain unsupported for broad proxy non-exposure
claims.

## Release assets

The public GitHub release mirror is:

```bash
https://github.com/jmagly/agentic-sandbox/releases/tag/<version>
```

Expected asset families for current production releases:

- `agentic-sandbox-<version>-x86_64-linux-gnu.tar.gz`
- `agentic-sandbox-<version>-x86_64-linux-musl.tar.gz`
- `agentic-sandbox-<version>-aarch64-linux-gnu.tar.gz`
- `agentic-sandbox_<version-without-v>-1_amd64.deb`
- `agentic-sandbox-<version-without-v>-1.x86_64.rpm`
- `agentic-sandbox-install.sh`
- `SHA256SUMS`
- `SHA256SUMS-linux-packages`
- Per-file `*.sha256` sidecars
- Optional `*.asc` detached signatures
- Optional `*.sbom.cdx.json` CycloneDX SBOMs

Older source-only releases may not have binary artifacts, package assets,
signatures, SBOMs, or image tags. Treat each release independently.

Darwin/macOS artifacts are deferred from the current public release matrix and
are not required for production release verification.

## Checksum verification

Download the canonical checksum manifest and the artifacts you plan to use:

```bash
VERSION=v2026.6.28
BASE="https://github.com/jmagly/agentic-sandbox/releases/download/${VERSION}"

curl -fLO "${BASE}/SHA256SUMS"
curl -fLO "${BASE}/agentic-sandbox-${VERSION}-x86_64-linux-gnu.tar.gz"
curl -fLO "${BASE}/agentic-sandbox_${VERSION#v}-1_amd64.deb"
curl -fLO "${BASE}/agentic-sandbox-${VERSION#v}-1.x86_64.rpm"
curl -fLO "${BASE}/agentic-sandbox-install.sh"

sha256sum -c --ignore-missing SHA256SUMS
```

To verify one sidecar instead of the aggregate manifest:

```bash
curl -fLO "${BASE}/agentic-sandbox-${VERSION}-x86_64-linux-gnu.tar.gz.sha256"
sha256sum -c "agentic-sandbox-${VERSION}-x86_64-linux-gnu.tar.gz.sha256"
```

Linux packages also have a package-specific manifest:

```bash
curl -fLO "${BASE}/SHA256SUMS-linux-packages"
sha256sum -c --ignore-missing SHA256SUMS-linux-packages
```

Any checksum mismatch is a hard failure. Delete the artifact, re-download it,
and do not install or run it unless the manifest check passes.

## Installer verification

The one-line installer downloads the selected `.deb` or `.rpm` and verifies it
against `SHA256SUMS-linux-packages` before install.

```bash
curl -fsSL "${BASE}/agentic-sandbox-install.sh" \
  | bash -s -- --version "${VERSION}" --dry-run
```

For local package testing:

```bash
bash agentic-sandbox-install.sh \
  --local-package "agentic-sandbox_${VERSION#v}-1_amd64.deb" \
  --dry-run

bash agentic-sandbox-install.sh \
  --local-package "agentic-sandbox-${VERSION#v}-1.x86_64.rpm" \
  --dry-run
```

Dry-run success means the installer resolved and validated inputs. It does not
install the package.

## Package inspection

Inspect package metadata before installing:

```bash
dpkg-deb --info "agentic-sandbox_${VERSION#v}-1_amd64.deb"
dpkg-deb --contents "agentic-sandbox_${VERSION#v}-1_amd64.deb" \
  | grep -E '/usr/bin/(agentic-mgmt|agent-client|sandboxctl|agentic-sandbox)$'

mkdir -p /tmp/agentic-sandbox-rpmdb
rpm --dbpath /tmp/agentic-sandbox-rpmdb \
  -qip "agentic-sandbox-${VERSION#v}-1.x86_64.rpm"
rpm --dbpath /tmp/agentic-sandbox-rpmdb \
  -qlp "agentic-sandbox-${VERSION#v}-1.x86_64.rpm" \
  | grep -E '^/usr/bin/(agentic-mgmt|agent-client|sandboxctl|agentic-sandbox)$'
```

Then install directly if desired:

```bash
sudo apt-get install "./agentic-sandbox_${VERSION#v}-1_amd64.deb"
sudo dnf install "./agentic-sandbox-${VERSION#v}-1.x86_64.rpm"
```

## Detached signatures

Detached signatures are optional and appear as `*.asc` assets when the release
signing key is configured. Since v2026.7 the release key is held in OpenBao
(rca-g2) rather than a CI secret and fetched ephemerally at signing time; the
key identity is unchanged for verifiers.

**Expected release-signing key**

| Field | Value |
|---|---|
| Fingerprint | `FE9272F0BC5781E1DE77FAAA719AB63879E84CE8` |
| Key ID | `719AB63879E84CE8` |

Treat a signature as identity evidence only when the imported key's fingerprint
matches the value above.

Download the artifact, its signature, and the expected public key or fingerprint
published for that release:

```bash
curl -fLO "${BASE}/agentic-sandbox-${VERSION}-x86_64-linux-gnu.tar.gz"
curl -fLO "${BASE}/agentic-sandbox-${VERSION}-x86_64-linux-gnu.tar.gz.asc"

gpg --import agentic-sandbox-release-key.asc
gpg --fingerprint
gpg --verify \
  "agentic-sandbox-${VERSION}-x86_64-linux-gnu.tar.gz.asc" \
  "agentic-sandbox-${VERSION}-x86_64-linux-gnu.tar.gz"
```

Only treat a GPG signature as identity evidence when the key fingerprint
matches the fingerprint published in the release notes or another trusted
project channel. If no `*.asc` asset or expected fingerprint is published, the
release should be described as checksum-verifiable but not GPG-signed.

## Container images and digests

Current release images are mirrored to GHCR with these names:

```text
ghcr.io/<owner>/agentic-sandbox-mgmt:<version>
ghcr.io/<owner>/agentic-sandbox-agent-client:<version>
ghcr.io/<owner>/agentic-sandbox-agent:<version>
ghcr.io/<owner>/agentic-sandbox-claude:<version>
ghcr.io/<owner>/agentic-sandbox-codex:<version>
ghcr.io/<owner>/agentic-sandbox-opencode:<version>
ghcr.io/<owner>/agentic-sandbox-automation-control:<version>
```

Inspect and pin the digest:

```bash
OWNER=jmagly
IMAGE="ghcr.io/${OWNER}/agentic-sandbox-mgmt:${VERSION}"

docker pull "${IMAGE}"
docker image inspect "${IMAGE}" --format '{{index .RepoDigests 0}}'
docker buildx imagetools inspect "${IMAGE}"
```

For deployment, prefer the immutable digest form printed by the registry:

```text
ghcr.io/jmagly/agentic-sandbox-mgmt@sha256:<digest>
```

## Cosign verification

The current workflow signs images with key-backed cosign when `COSIGN_KEY` is
configured. It does not currently claim keyless Sigstore/Fulcio identity or an
OIDC issuer constraint.

When a release publishes the expected cosign public key:

```bash
cosign verify --key cosign.pub "ghcr.io/jmagly/agentic-sandbox-mgmt:${VERSION}"
```

If the project later moves to keyless signing, verify both identity and issuer:

```bash
cosign verify \
  --certificate-identity "<expected-identity>" \
  --certificate-oidc-issuer "<expected-issuer>" \
  "ghcr.io/jmagly/agentic-sandbox-mgmt:${VERSION}"
```

Do not treat a release as cosign-verified unless the expected public key or
keyless identity constraints are published for that release.

## SBOMs

SBOM assets use CycloneDX JSON and are attached when the sign/SBOM job runs.
Tarball SBOM names follow:

```text
agentic-sandbox-<version>-<target>.sbom.cdx.json
```

Image SBOM names follow:

```text
agentic-sandbox-<image>-<version>.image.sbom.cdx.json
```

Download and inspect an SBOM:

```bash
curl -fLO "${BASE}/agentic-sandbox-${VERSION}-x86_64-linux-gnu.sbom.cdx.json"
jq '.bomFormat, .specVersion, (.components | length)' \
  "agentic-sandbox-${VERSION}-x86_64-linux-gnu.sbom.cdx.json"
```

An SBOM is an inventory. It is not a vulnerability scan result and does not
prove the artifact is safe.

## SLSA status

Agentic Sandbox does not currently claim a SLSA level for `v2026.6.28`.

Current partial alignment:

- Tag releases run a pre-release gate against Cargo versions and CHANGELOG.
- Release artifacts include aggregate and per-file checksums.
- GHCR publication is release-blocking for production tags.
- SBOM generation is wired through the sign/SBOM job.
- GPG and cosign signing are wired when release signing secrets are configured.

Current gaps before a SLSA claim:

- No published SLSA provenance attestation for release subjects.
- No in-toto layout or attestation verification guide.
- No committed public signing key/fingerprint policy for all releases.
- No documented keyless Sigstore identity and issuer constraints.

Use "SLSA-aligned release controls are in progress" rather than "SLSA Level N"
until those gaps are closed and independently verified.

## VM Base Image Provenance

QEMU base image provenance is local-operator evidence, not a published SLSA
attestation. The operator-controlled trust chain is:

- `images/qemu/iso-pins.json` pins Ubuntu ISO sha256 values derived from
  GPG-verified upstream `SHA256SUMS`.
- `images/qemu/build-base-image.sh` fails before `virt-install` when the local
  ISO does not match the pin.
- `/mnt/ops/base-images/manifest.json` records built qcow2 sha256 values.
- `images/qemu/provision-vm.sh` verifies the qcow2 manifest before overlay
  creation and records base image, cloud-init seed ISO, and loadout manifest
  hashes in each VM's `vm-info.json`.

Residual assumptions: the host filesystem and local GPG keyring are trusted,
and the retained cloud-init seed ISO remains sensitive until bootstrap values
inside it expire or the VM is retired.

## Failure behavior

Stop and investigate when any of these occur:

- `sha256sum -c` reports `FAILED`.
- `gpg --verify` reports a bad signature or an unexpected key fingerprint.
- `cosign verify` fails or verifies against a different key/identity.
- A release note claims SBOMs or signatures but the corresponding assets are
  missing.
- A container tag resolves to a digest that differs from the digest recorded in
  the release evidence.

Safe response:

1. Delete the failed download.
2. Re-download from the release page.
3. Re-run the verification command.
4. If it still fails, do not install or run the artifact; file an issue with
   the tag, asset name, digest/checksum observed, and command output.
