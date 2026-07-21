# macOS Apple Silicon package contract

The macOS release surface is a signed Installer package (`.pkg`) inside a
signed, notarized, stapled disk image (`.dmg`). The `.pkg` is also retained as a
standalone build artifact so CI can inspect its signature and payload before
the DMG is assembled.

## Current payload and support status

The first package contains only these Apple Silicon client tools:

- `/usr/local/bin/sandboxctl` (with the `agentic-sandbox` compatibility link)
- `/usr/local/bin/agent-client`

It does not contain `agentic-mgmt`, a VM provider, launchd services, or runtime
configuration. Packaging these client tools proves the release and trust path;
it does not promote macOS runtime support. Full management packaging remains
blocked on the Apple `container` feasibility and provider work in #438, #488,
and #489.

## Production trust requirements

Production artifacts fail closed unless all of these steps pass:

1. Both Mach-O executables are signed with a Developer ID Application identity,
   hardened runtime, and a trusted timestamp.
2. The Installer package is signed with a Developer ID Installer identity and
   its signing tool's default trusted timestamp, then submitted to Apple's
   notary service.
3. The notarization ticket is stapled to the `.pkg`, and `pkgutil`, `stapler`,
   and Gatekeeper verification pass.
4. The DMG containing the stapled package is signed with the Developer ID
   Application identity, notarized, stapled, and Gatekeeper-assessed.
5. SHA-256 sidecars and `SHA256SUMS-macos` cover both final artifacts.

Ad-hoc signing and an unstapled artifact are not production substitutes. The
HotM workflow is the reference for the Linux-runner-to-macOS-builder topology
and DMG publication shape, not for production signing policy.

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

After producing native `aarch64-apple-darwin` `sandboxctl` and `agent-client`
binaries in a staging directory:

```bash
scripts/package-macos.sh \
  --version "${RELEASE_TAG}" \
  --source-dir build-staging/aarch64-apple-darwin \
  --out-dir dist/macos
```

Do not enable this as a production-tag prerequisite until an eligible Apple
builder has the certificates and notary profile, and an operator has approved a
real signing/notarization proof. Once promoted, missing Apple build or signing
capability must fail the production release rather than silently omit the DMG.
