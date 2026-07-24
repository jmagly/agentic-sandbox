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

usage() {
  cat <<'EOF'
Usage: scripts/package-macos.sh --version <version> --source-dir <dir> [options]

Options:
  --mode <preview|production>  Credential-free package or production promotion
  --out-dir <dir>             Output directory (default: dist/macos)
  --approved-payload-manifest <path>
                              Required in production; exact preview manifest

The source directory must contain executable Apple Silicon Mach-O binaries:
agentic-mgmt, agentic-host-runtime-daemon, sandboxctl, and agent-client.

Preview mode builds an unsigned package, payload manifest, and checksum
sidecar. It never signs, notarizes, staples, loads launchd services, or reads
credentials. Preview artifacts are not production releases.

Production mode additionally requires these non-secret identifiers:
  APPLE_DEVELOPER_ID_APPLICATION  Developer ID Application identity
  APPLE_DEVELOPER_ID_INSTALLER    Developer ID Installer identity
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
  require_env APPLE_DEVELOPER_ID_APPLICATION
  require_env APPLE_DEVELOPER_ID_INSTALLER
  require_env APPLE_NOTARY_KEYCHAIN_PROFILE
  for command_name in cmp codesign hdiutil jq pkgutil spctl xcrun; do
    require_command "$command_name"
  done
fi

for binary in agentic-mgmt agentic-host-runtime-daemon sandboxctl agent-client; do
  [ -x "$SOURCE_DIR/$binary" ] || { echo "required executable not found: $SOURCE_DIR/$binary" >&2; exit 1; }
  file "$SOURCE_DIR/$binary" | grep -Eq 'Mach-O.*arm64|Mach-O 64-bit.*arm64' \
    || { echo "required Apple Silicon Mach-O binary not found: $SOURCE_DIR/$binary" >&2; exit 1; }
done

TMP_ROOT="$(mktemp -d -t agentic-macos-package.XXXXXX)"
trap 'rm -rf "$TMP_ROOT"' EXIT

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
  unsigned_manifest="$TMP_ROOT/approved-input-payload-manifest.tsv"
  generate_payload_manifest "$unsigned_manifest"
  cmp -s "$APPROVED_PAYLOAD_MANIFEST" "$unsigned_manifest" || {
    echo "production payload does not match the approved credential-free manifest" >&2
    exit 1
  }
  for binary in \
    agentic-mgmt \
    agentic-host-runtime-daemon \
    sandboxctl \
    agent-client; do
    codesign --force --options runtime --timestamp \
      --sign "$APPLE_DEVELOPER_ID_APPLICATION" \
      "$PAYLOAD/usr/local/bin/$binary"
    codesign --verify --strict --verbose=2 "$PAYLOAD/usr/local/bin/$binary"
  done
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
pkgutil --check-signature "$PKG_PATH"
spctl --assess --type install --verbose=2 "$PKG_PATH"

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
signed_manifest_digest="$(shasum -a 256 "$MANIFEST_PATH" | awk '{print $1}')"
pkg_digest="$(shasum -a 256 "$PKG_PATH" | awk '{print $1}')"
dmg_digest="$(shasum -a 256 "$DMG_PATH" | awk '{print $1}')"
jq -n \
  --arg schema_version "1" \
  --arg version "$VERSION" \
  --arg package_identifier "io.aiwg.agentic-sandbox" \
  --arg approved_payload_manifest_sha256 "$approved_manifest_digest" \
  --arg signed_payload_manifest_sha256 "$signed_manifest_digest" \
  --arg pkg_name "$(basename "$PKG_PATH")" \
  --arg pkg_sha256 "$pkg_digest" \
  --arg dmg_name "$(basename "$DMG_PATH")" \
  --arg dmg_sha256 "$dmg_digest" \
  '{
    schema_version: $schema_version,
    version: $version,
    architecture: "aarch64-apple-darwin",
    package_identifier: $package_identifier,
    approved_payload_manifest_sha256: $approved_payload_manifest_sha256,
    signed_payload_manifest_sha256: $signed_payload_manifest_sha256,
    artifacts: [
      {name: $pkg_name, sha256: $pkg_sha256, verified: ["developer-id-installer", "notarized", "stapled", "gatekeeper"]},
      {name: $dmg_name, sha256: $dmg_sha256, verified: ["developer-id-application", "notarized", "stapled", "gatekeeper"]}
    ],
    credential_contents_retained: false
  }' > "$EVIDENCE_PATH"
chmod 0644 "$EVIDENCE_PATH"

printf 'macOS package verification passed:\n  %s\n  %s\n  %s\n' \
  "$PKG_PATH" "$DMG_PATH" "$EVIDENCE_PATH"
