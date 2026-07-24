# macOS Apple Silicon package contract

The macOS release surface is a signed Installer package (`.pkg`) inside a
signed, notarized, stapled disk image (`.dmg`). The `.pkg` is also retained as a
standalone build artifact so CI can inspect its signature and payload before
the DMG is assembled.

## Payload and support status

The Apple Silicon package contains:

- `/usr/local/bin/agentic-mgmt`
- `/usr/local/bin/agentic-host-runtime-daemon`
- `/usr/local/bin/sandboxctl` (with the `agentic-sandbox` compatibility link)
- `/usr/local/bin/agent-client`
- an inert per-user LaunchAgent template and credential-free renderer;
- an example management/host/Keychain configuration;
- the bounded package uninstaller;
- payload, support, and Keychain/host-runtime documentation.

Package construction and installation do not load, enable, or start the
LaunchAgent. Native host execution remains explicit opt-in because it grants
agents the ambient permissions of the daemon user. Docker Desktop is the
supported macOS container path. The package does not contain libvirt, KVM,
Cloud Hypervisor, VFIO/GPU, systemd, or an Apple `container` provider.

The stable package identifier is `io.aiwg.agentic-sandbox`. Upgrades replace
the same package-owned paths and never activate launchd or delete per-user
state. `PAYLOAD-MANIFEST.tsv` pins every installed file digest/mode and symlink
target. The package-owned uninstaller removes only those paths; it preserves
user-rendered LaunchAgents, Application Support state, workspaces, TLS
material, and Keychain items.

## Credential-free preview

Preview mode requires no signing or notarization identity:

```bash
scripts/package-macos.sh \
  --mode preview \
  --version "${VERSION}" \
  --source-dir build-staging/aarch64-apple-darwin \
  --out-dir dist/macos
```

It produces:

- `agentic-sandbox-v<VERSION>-aarch64-darwin-preview.pkg`;
- a package checksum sidecar;
- `agentic-sandbox-v<VERSION>-aarch64-darwin.payload-manifest.tsv`.

On mutsu, the serialized validation lane expands this package into an isolated
temporary root, verifies every manifest entry, and runs the package-owned
uninstaller against that root. It never invokes the system installer or
mutates persistent locations. A preview artifact is not signed, notarized,
stapled, or eligible for production publication.

## Production trust requirements

Production artifacts fail closed unless all of these steps pass:

1. All four Mach-O executables are signed with a Developer ID Application
   identity, hardened runtime, and a trusted timestamp.
2. The Installer package is signed with a Developer ID Installer identity and
   its signing tool's default trusted timestamp, then submitted to Apple's
   notary service.
3. The notarization ticket is stapled to the `.pkg`, and `pkgutil`, `stapler`,
   and Gatekeeper verification pass.
4. The DMG containing the stapled package is signed with the Developer ID
   Application identity, notarized, stapled, and Gatekeeper-assessed.
5. SHA-256 sidecars and `SHA256SUMS-macos` cover both final artifacts.

Ad-hoc signing and an unstapled artifact are not production substitutes. The
HotM and Carbonyl workflows are references for the
Linux-runner-to-macOS-builder topology and `.pkg`/DMG publication shape, not
for production signing policy. Carbonyl's current macOS artifacts are unsigned;
this package contract deliberately requires Developer ID signing,
notarization, and stapling before production promotion.

## Credential boundary

`scripts/package-macos.sh` accepts only certificate identity strings and a
`notarytool` Keychain profile name through environment variables. Certificate
private keys and notarization credentials must already be provisioned in the
macOS Keychain. The script does not accept secret values on its command line,
read credential files, import identities, or print credentials.

Required environment:

```text
APPLE_DEVELOPER_ID_APPLICATION
APPLE_DEVELOPER_ID_INSTALLER
APPLE_NOTARY_KEYCHAIN_PROFILE
```

The operator-owned provisioning step should create the notary profile with
`xcrun notarytool store-credentials` outside the repository and CI logs.

## Build invocation

After producing native `aarch64-apple-darwin` `agentic-mgmt`,
`agentic-host-runtime-daemon`, `sandboxctl`, and `agent-client` binaries in a
staging directory:

```bash
scripts/package-macos.sh \
  --mode production \
  --version "${RELEASE_TAG}" \
  --source-dir build-staging/aarch64-apple-darwin \
  --approved-payload-manifest \
    dist/macos/agentic-sandbox-v${VERSION}-aarch64-darwin.payload-manifest.tsv \
  --out-dir dist/macos
```

Production mode regenerates the unsigned payload manifest before any signing
operation and requires an exact byte-for-byte match with the approved preview
manifest. Payload drift stops promotion. After public verification succeeds,
the packager emits a sanitized `release-evidence.json` containing the approved
and signed payload-manifest digests, final artifact digests, package identity,
and the signature/notarization/stapling/Gatekeeper gates that passed. It never
contains Keychain or notarization credential contents.

Do not enable this as a production-tag prerequisite until an eligible Apple
builder has the certificates and notary profile, and an operator has approved a
real signing/notarization proof. Once promoted, missing Apple build or signing
capability must fail the production release rather than silently omit the DMG.

## Opt-in host runtime after installation

Render the packaged template into the logged-in user's LaunchAgents directory:

```bash
/usr/local/libexec/agentic-sandbox/render-macos-launch-agent \
  --output "$HOME/Library/LaunchAgents/io.aiwg.agentic-sandbox.host-runtime.plist"
plutil -lint "$HOME/Library/LaunchAgents/io.aiwg.agentic-sandbox.host-runtime.plist"
```

Review the full-host-access warning and rendered paths before explicitly using
`launchctl bootstrap`. Installation alone never enables the service.

## Uninstall

First boot out any user LaunchAgent that the operator explicitly enabled.
Then remove only package-owned paths:

```bash
sudo /usr/local/libexec/agentic-sandbox/uninstall-macos --confirm
```

This deliberately leaves user state, workspaces, TLS material, Keychain items,
and user-rendered LaunchAgent files untouched. Follow the witnessed Keychain/CA
rotation procedure only when those identities must also be retired.

## Enterprise composition boundary

The public package remains complete and independently verifiable. Licensed
enterprise components ship as separate packages or as constituents of an outer
distribution manifest. They must pin the exact public artifact digest and may
not unpack/repack or otherwise mutate the signed/notarized public package.
ADR-033 defines the public/private repository and release relationship.
