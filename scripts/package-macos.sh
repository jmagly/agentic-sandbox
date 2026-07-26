#!/usr/bin/env bash
# Build the Apple Silicon runtime package and optionally promote it through the
# operator-controlled Developer ID and notarization trust chain.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION=""
SOURCE_DIR=""
OUT_DIR="$ROOT/dist/macos"
MODE="production"
APPROVED_PAYLOAD_MANIFEST=""
APPROVED_PREVIEW_PACKAGE=""
OPERATOR_APPROVAL_REF=""
SOURCE_COMMIT=""
RELEASE_TAG=""

usage() {
  cat <<'EOF'
Usage: scripts/package-macos.sh --version <version> --source-dir <dir> [options]

Options:
  --mode <preview|production>  Credential-free package or production promotion
  --out-dir <dir>             Output directory (default: dist/macos)
  --approved-payload-manifest <path>
                              Required in production; exact preview manifest
  --approved-preview-package <path>
                              Required in production; exact preview package
  --operator-approval-ref <ref>
                              Required non-secret witnessed approval reference
  --source-commit <sha>       Required exact 40-character source commit
  --release-tag <tag>         Required tag; must match --version

The source directory must contain executable Apple Silicon Mach-O binaries:
agentic-mgmt, agentic-host-runtime-daemon, sandboxctl, and agent-client.

Preview mode builds an unsigned package, payload manifest, and checksum
sidecar. It never signs, notarizes, staples, loads launchd services, or reads
credentials. Preview artifacts are not production releases.

Production mode additionally requires these non-secret identifiers:
  APPLE_DEVELOPER_ID_APPLICATION  Developer ID Application identity
  APPLE_DEVELOPER_ID_INSTALLER    Developer ID Installer identity
  APPLE_DEVELOPER_ID_TEAM_ID      exact ten-character Apple Team ID
  APPLE_NOTARY_KEYCHAIN_PROFILE   notarytool Keychain profile name

The certificate private keys and notarization credentials must already exist
in the macOS Keychain. This script does not import, print, or accept them on
the command line.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:-}"; shift 2 ;;
    --source-dir) SOURCE_DIR="${2:-}"; shift 2 ;;
    --out-dir) OUT_DIR="${2:-}"; shift 2 ;;
    --mode) MODE="${2:-}"; shift 2 ;;
    --approved-payload-manifest) APPROVED_PAYLOAD_MANIFEST="${2:-}"; shift 2 ;;
    --approved-preview-package) APPROVED_PREVIEW_PACKAGE="${2:-}"; shift 2 ;;
    --operator-approval-ref) OPERATOR_APPROVAL_REF="${2:-}"; shift 2 ;;
    --source-commit) SOURCE_COMMIT="${2:-}"; shift 2 ;;
    --release-tag) RELEASE_TAG="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[ -n "$VERSION" ] || { echo "missing --version" >&2; usage >&2; exit 2; }
[ -n "$SOURCE_DIR" ] || { echo "missing --source-dir" >&2; usage >&2; exit 2; }
VERSION="${VERSION#v}"

case "$VERSION" in
  *[!0-9.]*|"") echo "package version must be CalVer-like digits/dots: $VERSION" >&2; exit 2 ;;
esac
case "$MODE" in
  preview|production) ;;
  *) echo "--mode must be preview or production" >&2; exit 2 ;;
esac

if [ "${AGENTIC_MACOS_PACKAGING_TEST_MODE:-0}" != "1" ]; then
  [ "$(uname -s)" = "Darwin" ] || { echo "macOS packaging requires a Darwin host" >&2; exit 1; }
  [ "$(uname -m)" = "arm64" ] || { echo "macOS packaging requires Apple Silicon" >&2; exit 1; }
elif [ "${CI:-false}" != "true" ]; then
  echo "AGENTIC_MACOS_PACKAGING_TEST_MODE is restricted to CI fixture tests" >&2
  exit 1
fi

require_env() {
  local name="$1"
  [ -n "${!name:-}" ] || { echo "required environment variable is not set: $name" >&2; exit 1; }
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || { echo "required command not found: $1" >&2; exit 1; }
}

for command_name in awk file find install pkgbuild readlink shasum stat; do
  require_command "$command_name"
done

if [ "$MODE" = "production" ]; then
  [ -f "$APPROVED_PAYLOAD_MANIFEST" ] \
    || { echo "production mode requires --approved-payload-manifest" >&2; exit 1; }
  [ -f "$APPROVED_PREVIEW_PACKAGE" ] \
    || { echo "production mode requires --approved-preview-package" >&2; exit 1; }
  [[ "$OPERATOR_APPROVAL_REF" =~ ^[A-Za-z0-9][A-Za-z0-9._:/#@+-]{2,255}$ ]] \
    || { echo "production mode requires a valid public --operator-approval-ref" >&2; exit 1; }
  [[ "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] \
    || { echo "production mode requires an exact lowercase --source-commit" >&2; exit 1; }
  [[ "$RELEASE_TAG" == "v${VERSION}" ]] \
    || { echo "--release-tag must exactly match the package version" >&2; exit 1; }
  require_env APPLE_DEVELOPER_ID_APPLICATION
  require_env APPLE_DEVELOPER_ID_INSTALLER
  require_env APPLE_DEVELOPER_ID_TEAM_ID
  require_env APPLE_NOTARY_KEYCHAIN_PROFILE
  for command_name in cmp codesign hdiutil jq pkgutil plutil python3 security spctl xcrun; do
    require_command "$command_name"
  done
fi

for binary in agentic-mgmt agentic-host-runtime-daemon sandboxctl agent-client; do
  [ -x "$SOURCE_DIR/$binary" ] || { echo "required executable not found: $SOURCE_DIR/$binary" >&2; exit 1; }
  file "$SOURCE_DIR/$binary" | grep -Eq 'Mach-O.*arm64|Mach-O 64-bit.*arm64' \
    || { echo "required Apple Silicon Mach-O binary not found: $SOURCE_DIR/$binary" >&2; exit 1; }
done

TMP_ROOT="$(mktemp -d -t agentic-macos-package.XXXXXX)"
PRODUCTION_COMPLETE=0
QUARANTINE_CANDIDATES=()
cleanup() {
  status=$?
  trap - EXIT
  if [[ "$MODE" == "production" && "$PRODUCTION_COMPLETE" != "1" ]]; then
    quarantine_root="$OUT_DIR/quarantine"
    quarantined=0
    for candidate in "${QUARANTINE_CANDIDATES[@]}"; do
      if [[ -e "$candidate" ]]; then
        mkdir -p "$quarantine_root"
        mv "$candidate" "$quarantine_root/"
        quarantined=1
      fi
    done
    if [[ "$quarantined" == "1" ]]; then
      jq -n \
        --arg schema_version "agentic.macos-release-quarantine.v1" \
        --arg source_commit "$SOURCE_COMMIT" \
        --arg release_tag "$RELEASE_TAG" \
        --arg operator_approval_ref "$OPERATOR_APPROVAL_REF" \
        '{
          schema_version: $schema_version,
          source_commit: $source_commit,
          release_tag: $release_tag,
          operator_approval_ref: $operator_approval_ref,
          state: "quarantined",
          credential_contents_retained: false
        }' > "$quarantine_root/quarantine.json"
      chmod 0644 "$quarantine_root/quarantine.json"
      echo "production ceremony aborted; partial artifacts were quarantined" >&2
    fi
  fi
  rm -rf "$TMP_ROOT"
  exit "$status"
}
trap cleanup EXIT INT TERM

PAYLOAD="$TMP_ROOT/payload"
DMG_ROOT="$TMP_ROOT/dmg-root"
mkdir -p \
  "$PAYLOAD/usr/local/bin" \
  "$PAYLOAD/usr/local/libexec/agentic-sandbox" \
  "$PAYLOAD/usr/local/share/agentic-sandbox/config" \
  "$PAYLOAD/usr/local/share/agentic-sandbox/launchd" \
  "$PAYLOAD/usr/local/share/doc/agentic-sandbox" \
  "$DMG_ROOT"

install -m 0755 "$SOURCE_DIR/agentic-mgmt" "$PAYLOAD/usr/local/bin/agentic-mgmt"
install -m 0755 \
  "$SOURCE_DIR/agentic-host-runtime-daemon" \
  "$PAYLOAD/usr/local/bin/agentic-host-runtime-daemon"
install -m 0755 "$SOURCE_DIR/sandboxctl" "$PAYLOAD/usr/local/bin/sandboxctl"
install -m 0755 "$SOURCE_DIR/agent-client" "$PAYLOAD/usr/local/bin/agent-client"
ln -s sandboxctl "$PAYLOAD/usr/local/bin/agentic-sandbox"
install -m 0755 \
  "$ROOT/scripts/render-macos-launch-agent.sh" \
  "$PAYLOAD/usr/local/libexec/agentic-sandbox/render-macos-launch-agent"
install -m 0755 \
  "$ROOT/scripts/uninstall-macos.sh" \
  "$PAYLOAD/usr/local/libexec/agentic-sandbox/uninstall-macos"
install -m 0644 \
  "$ROOT/deploy/launchd/io.aiwg.agentic-sandbox.host-runtime.plist" \
  "$PAYLOAD/usr/local/share/agentic-sandbox/launchd/io.aiwg.agentic-sandbox.host-runtime.plist"
install -m 0644 \
  "$ROOT/deploy/packaging/macos/agentic-sandbox.env.example" \
  "$PAYLOAD/usr/local/share/agentic-sandbox/config/agentic-sandbox.env.example"
install -m 0644 "$ROOT/README.md" "$PAYLOAD/usr/local/share/doc/agentic-sandbox/README.md"
install -m 0644 "$ROOT/LICENSE" "$PAYLOAD/usr/local/share/doc/agentic-sandbox/LICENSE"
install -m 0644 "$ROOT/CHANGELOG.md" "$PAYLOAD/usr/local/share/doc/agentic-sandbox/CHANGELOG.md"
install -m 0644 \
  "$ROOT/docs/releases/macos-package.md" \
  "$PAYLOAD/usr/local/share/doc/agentic-sandbox/README-macOS.md"
install -m 0644 \
  "$ROOT/docs/operations/macos-host-runtime-keychain.md" \
  "$PAYLOAD/usr/local/share/doc/agentic-sandbox/HOST-RUNTIME-KEYCHAIN.md"

cat > "$PAYLOAD/usr/local/share/doc/agentic-sandbox/MACOS_SCOPE.md" <<'EOF'
# macOS package scope

This package contains the Apple Silicon management server, native host-runtime
daemon, sandboxctl, and agent-client. It also installs an inert user
LaunchAgent template and renderer. Package construction and installation do
not bootstrap, enable, or start the LaunchAgent. Native host execution grants
agents the ambient permissions of the daemon user and must be explicitly
enabled after reviewing the full-host-access warning.

Docker Desktop is the supported container runtime on macOS. Linux-only
libvirt, KVM, Cloud Hypervisor, VFIO, GPU passthrough, cgroups, namespaces,
seccomp, and systemd capabilities are not supplied by this package.
EOF

generate_payload_manifest() {
  local output="$1"
  (
    cd "$PAYLOAD"
    find . \( -type f -o -type l \) \
      ! -path './usr/local/share/doc/agentic-sandbox/PAYLOAD-MANIFEST.tsv' \
      -print |
      LC_ALL=C sort |
      while IFS= read -r relative_path; do
        relative_path="${relative_path#./}"
        if [ -L "$relative_path" ]; then
          printf 'symlink\t-\t%s\t%s\n' \
            "$relative_path" "$(readlink "$relative_path")"
        else
          if [ "$(uname -s)" = "Darwin" ]; then
            mode="$(stat -f '%Lp' "$relative_path")"
          else
            mode="$(stat -c '%a' "$relative_path")"
          fi
          digest="$(shasum -a 256 "$relative_path" | awk '{print $1}')"
          printf 'file\t%s\t%s\t%s\n' "$mode" "$relative_path" "$digest"
        fi
      done
  ) > "$output"
  chmod 0644 "$output"
}

if [ "$MODE" = "production" ]; then
  expected_preview_name="agentic-sandbox-v${VERSION}-aarch64-darwin-preview.pkg"
  [[ "$(basename "$APPROVED_PREVIEW_PACKAGE")" == "$expected_preview_name" ]] \
    || { echo "approved preview package name does not match the release" >&2; exit 1; }
  unsigned_manifest="$TMP_ROOT/approved-input-payload-manifest.tsv"
  generate_payload_manifest "$unsigned_manifest"
  cmp -s "$APPROVED_PAYLOAD_MANIFEST" "$unsigned_manifest" || {
    echo "production payload does not match the approved credential-free manifest" >&2
    exit 1
  }
  PREFLIGHT_PATH="$TMP_ROOT/release-preflight.json"
  "$ROOT/scripts/macos-release-preflight.sh" --output "$PREFLIGHT_PATH"
  verify_application_signature_metadata() {
    local signed_path="$1"
    local metadata
    metadata="$TMP_ROOT/codesign-metadata-$(basename "$signed_path")"
    if ! codesign -d --verbose=4 "$signed_path" >/dev/null 2>"$metadata"; then
      echo "unable to read public Developer ID signature metadata" >&2
      exit 1
    fi
    grep -Fqx "Authority=${APPLE_DEVELOPER_ID_APPLICATION}" "$metadata" \
      || { echo "signed artifact does not use the expected Application selector" >&2; exit 1; }
    grep -Fqx "TeamIdentifier=${APPLE_DEVELOPER_ID_TEAM_ID}" "$metadata" \
      || { echo "signed artifact does not use the expected Team ID" >&2; exit 1; }
    grep -Eq '^Timestamp=.+' "$metadata" \
      || { echo "signed artifact lacks a trusted timestamp" >&2; exit 1; }
  }
  for binary in \
    agentic-mgmt \
    agentic-host-runtime-daemon \
    sandboxctl \
    agent-client; do
    codesign --force --options runtime --timestamp \
      --entitlements "$ROOT/deploy/packaging/macos/agentic-sandbox.entitlements.plist" \
      --sign "$APPLE_DEVELOPER_ID_APPLICATION" \
      "$PAYLOAD/usr/local/bin/$binary"
    codesign --verify --strict --verbose=2 "$PAYLOAD/usr/local/bin/$binary"
    verify_application_signature_metadata "$PAYLOAD/usr/local/bin/$binary"
  done
  ENTITLEMENT_RESULTS="$TMP_ROOT/entitlement-results.tsv"
  "$ROOT/scripts/verify-macos-entitlements.sh" \
    --expected "$ROOT/deploy/packaging/macos/agentic-sandbox.entitlements.plist" \
    --output "$ENTITLEMENT_RESULTS" \
    "$PAYLOAD/usr/local/bin/agentic-mgmt" \
    "$PAYLOAD/usr/local/bin/agentic-host-runtime-daemon" \
    "$PAYLOAD/usr/local/bin/sandboxctl" \
    "$PAYLOAD/usr/local/bin/agent-client"
fi

manifest_path="$PAYLOAD/usr/local/share/doc/agentic-sandbox/PAYLOAD-MANIFEST.tsv"
generate_payload_manifest "$manifest_path"

mkdir -p "$OUT_DIR"
BASE="agentic-sandbox-v${VERSION}-aarch64-darwin"
if [ "$MODE" = "preview" ]; then
  PKG_PATH="$OUT_DIR/${BASE}-preview.pkg"
else
  PKG_PATH="$OUT_DIR/${BASE}.pkg"
fi
DMG_PATH="$OUT_DIR/${BASE}.dmg"
SUMS_PATH="$OUT_DIR/SHA256SUMS-macos"
MANIFEST_PATH="$OUT_DIR/${BASE}.payload-manifest.tsv"
EVIDENCE_PATH="$OUT_DIR/${BASE}.release-evidence.json"
QUARANTINE_CANDIDATES=(
  "$PKG_PATH"
  "$DMG_PATH"
  "$SUMS_PATH"
  "$PKG_PATH.sha256"
  "$DMG_PATH.sha256"
  "$MANIFEST_PATH"
  "$EVIDENCE_PATH"
)
rm -f \
  "$PKG_PATH" \
  "$DMG_PATH" \
  "$SUMS_PATH" \
  "$PKG_PATH.sha256" \
  "$DMG_PATH.sha256" \
  "$MANIFEST_PATH" \
  "$EVIDENCE_PATH"

pkgbuild_args=(
  --root "$PAYLOAD"
  --identifier io.aiwg.agentic-sandbox
  --version "$VERSION"
  --install-location /
)
if [ "$MODE" = "production" ]; then
  pkgbuild_args+=(--sign "$APPLE_DEVELOPER_ID_INSTALLER")
fi
pkgbuild "${pkgbuild_args[@]}" "$PKG_PATH"
install -m 0644 "$manifest_path" "$MANIFEST_PATH"

if [ "$MODE" = "preview" ]; then
  (
    cd "$OUT_DIR"
    shasum -a 256 "$(basename "$PKG_PATH")" > "$(basename "$PKG_PATH").sha256"
  )
  printf 'macOS credential-free preview package built:\n  %s\n  %s\n' \
    "$PKG_PATH" "$MANIFEST_PATH"
  exit 0
fi

xcrun notarytool submit "$PKG_PATH" \
  --keychain-profile "$APPLE_NOTARY_KEYCHAIN_PROFILE" \
  --wait
xcrun stapler staple "$PKG_PATH"
xcrun stapler validate "$PKG_PATH"
pkg_signature_metadata="$TMP_ROOT/pkg-signature-metadata"
pkgutil --check-signature "$PKG_PATH" >"$pkg_signature_metadata" 2>&1
grep -Fq 'Status: signed by a certificate trusted by macOS' "$pkg_signature_metadata" \
  || { echo "Installer package signature is not trusted by macOS" >&2; exit 1; }
grep -Fq "$APPLE_DEVELOPER_ID_INSTALLER" "$pkg_signature_metadata" \
  || { echo "Installer package does not use the expected identity selector" >&2; exit 1; }
spctl --assess --type install --verbose=2 "$PKG_PATH"

receipt_root="$TMP_ROOT/package-receipt"
pkgutil --expand "$PKG_PATH" "$receipt_root"
package_info="$receipt_root/PackageInfo"
[[ -f "$package_info" ]] \
  || { echo "signed package receipt metadata is unavailable" >&2; exit 1; }
python3 - "$package_info" "$VERSION" <<'PY'
import sys
import xml.etree.ElementTree as ET

root = ET.parse(sys.argv[1]).getroot()
if root.attrib.get("identifier") != "io.aiwg.agentic-sandbox":
    raise SystemExit("signed package receipt identifier mismatch")
if root.attrib.get("version") != sys.argv[2]:
    raise SystemExit("signed package receipt version mismatch")
PY

install -m 0644 "$PKG_PATH" "$DMG_ROOT/$(basename "$PKG_PATH")"
install -m 0644 "$ROOT/docs/releases/macos-package.md" "$DMG_ROOT/README-macOS.md"
install -m 0644 "$ROOT/LICENSE" "$DMG_ROOT/LICENSE"

hdiutil create \
  -volname "Agentic Sandbox ${VERSION}" \
  -srcfolder "$DMG_ROOT" \
  -ov \
  -format UDZO \
  "$DMG_PATH"
codesign --force --timestamp \
  --sign "$APPLE_DEVELOPER_ID_APPLICATION" \
  "$DMG_PATH"
codesign --verify --strict --verbose=2 "$DMG_PATH"
verify_application_signature_metadata "$DMG_PATH"

xcrun notarytool submit "$DMG_PATH" \
  --keychain-profile "$APPLE_NOTARY_KEYCHAIN_PROFILE" \
  --wait
xcrun stapler staple "$DMG_PATH"
xcrun stapler validate "$DMG_PATH"
spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG_PATH"

(
  cd "$OUT_DIR"
  shasum -a 256 "$(basename "$PKG_PATH")" "$(basename "$DMG_PATH")" > "$(basename "$SUMS_PATH")"
  shasum -a 256 "$(basename "$PKG_PATH")" > "$(basename "$PKG_PATH").sha256"
  shasum -a 256 "$(basename "$DMG_PATH")" > "$(basename "$DMG_PATH").sha256"
)

approved_manifest_digest="$(shasum -a 256 "$APPROVED_PAYLOAD_MANIFEST" | awk '{print $1}')"
approved_preview_package_digest="$(shasum -a 256 "$APPROVED_PREVIEW_PACKAGE" | awk '{print $1}')"
signed_manifest_digest="$(shasum -a 256 "$MANIFEST_PATH" | awk '{print $1}')"
pkg_digest="$(shasum -a 256 "$PKG_PATH" | awk '{print $1}')"
dmg_digest="$(shasum -a 256 "$DMG_PATH" | awk '{print $1}')"
expected_entitlements_digest="$(
  shasum -a 256 "$ROOT/deploy/packaging/macos/agentic-sandbox.entitlements.plist" |
    awk '{print $1}'
)"
payloads_json="$(
  jq -Rn \
    --arg expected_entitlements_sha256 "$expected_entitlements_digest" \
    '[inputs | split("\t") | {
      path: ("usr/local/bin/" + .[0]),
      sha256: .[1],
      expected_entitlements_sha256: $expected_entitlements_sha256,
      entitlements_exact_match: (.[3] == "exact"),
      verified: [
        "developer-id-application",
        "hardened-runtime",
        "trusted-timestamp",
        "no-extra-entitlements"
      ]
    }]' < "$ENTITLEMENT_RESULTS"
)"
jq -e 'length == 4 and all(.entitlements_exact_match == true)' \
  <<<"$payloads_json" >/dev/null \
  || { echo "signed payload entitlement evidence is incomplete" >&2; exit 1; }
evidence_tmp="$TMP_ROOT/release-evidence.json"
jq -n \
  --arg schema_version "agentic.macos-release-evidence.v1" \
  --arg version "$VERSION" \
  --arg release_tag "$RELEASE_TAG" \
  --arg source_commit "$SOURCE_COMMIT" \
  --arg operator_approval_ref "$OPERATOR_APPROVAL_REF" \
  --arg team_id "$APPLE_DEVELOPER_ID_TEAM_ID" \
  --arg preview_pkg_name "$(basename "$APPROVED_PREVIEW_PACKAGE")" \
  --arg preview_pkg_sha256 "$approved_preview_package_digest" \
  --arg approved_manifest_name "$(basename "$APPROVED_PAYLOAD_MANIFEST")" \
  --arg approved_payload_manifest_sha256 "$approved_manifest_digest" \
  --arg signed_manifest_name "$(basename "$MANIFEST_PATH")" \
  --arg signed_payload_manifest_sha256 "$signed_manifest_digest" \
  --arg expected_entitlements_sha256 "$expected_entitlements_digest" \
  --argjson preflight "$(jq '{application_identity, installer_identity, notary_profile}' "$PREFLIGHT_PATH")" \
  --argjson payloads "$payloads_json" \
  --arg pkg_name "$(basename "$PKG_PATH")" \
  --arg pkg_sha256 "$pkg_digest" \
  --arg dmg_name "$(basename "$DMG_PATH")" \
  --arg dmg_sha256 "$dmg_digest" \
  '{
    schema_version: $schema_version,
    release: {
      version: $version,
      tag: $release_tag,
      source_commit: $source_commit,
      operator_approval_ref: $operator_approval_ref
    },
    package: {
      architecture: "aarch64-apple-darwin",
      identifier: "io.aiwg.agentic-sandbox",
      team_id: $team_id,
      approved_preview_package: {
        name: $preview_pkg_name,
        sha256: $preview_pkg_sha256
      },
      approved_preview_manifest: {
        name: $approved_manifest_name,
        sha256: $approved_payload_manifest_sha256
      },
      signed_payload_manifest: {
        name: $signed_manifest_name,
        sha256: $signed_payload_manifest_sha256
      },
      expected_entitlements_sha256: $expected_entitlements_sha256
    },
    preflight: $preflight,
    payloads: $payloads,
    artifacts: [
      {
        kind: "pkg",
        name: $pkg_name,
        sha256: $pkg_sha256,
        verified: [
          "developer-id-installer",
          "notarized",
          "stapled",
          "gatekeeper",
          "package-receipt"
        ]
      },
      {
        kind: "dmg",
        name: $dmg_name,
        sha256: $dmg_sha256,
        verified: [
          "developer-id-application",
          "notarized",
          "stapled",
          "gatekeeper"
        ]
      }
    ],
    ceremony: {
      outcome: "verified",
      artifact_quarantine: "clear"
    },
    credential_contents_retained: false
  }' > "$evidence_tmp"
"$ROOT/scripts/validate-macos-release-evidence.py" \
  "$evidence_tmp" \
  --expect-source-commit "$SOURCE_COMMIT" \
  --expect-tag "$RELEASE_TAG" \
  --expect-operator-approval-ref "$OPERATOR_APPROVAL_REF" \
  --expect-preview-manifest-sha256 "$approved_manifest_digest" \
  --expect-preview-package-sha256 "$approved_preview_package_digest"
install -m 0644 "$evidence_tmp" "$EVIDENCE_PATH"
chmod 0644 "$EVIDENCE_PATH"
PRODUCTION_COMPLETE=1

printf 'macOS package verification passed:\n  %s\n  %s\n  %s\n' \
  "$PKG_PATH" "$DMG_PATH" "$EVIDENCE_PATH"
