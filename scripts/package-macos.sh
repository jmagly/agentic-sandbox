#!/usr/bin/env bash
# Build, sign, notarize, and verify the Apple Silicon client-tools installer.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION=""
SOURCE_DIR=""
OUT_DIR="$ROOT/dist/macos"

usage() {
  cat <<'EOF'
Usage: scripts/package-macos.sh --version <version> --source-dir <dir> [--out-dir <dir>]

Builds a signed Installer package and a signed, notarized, stapled DMG for
Apple Silicon. The source directory must contain executable Mach-O arm64
sandboxctl and agent-client binaries.

Required environment (identifiers/profile names only; never secret values):
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

require_env APPLE_DEVELOPER_ID_APPLICATION
require_env APPLE_DEVELOPER_ID_INSTALLER
require_env APPLE_NOTARY_KEYCHAIN_PROFILE

for command_name in codesign file hdiutil install pkgbuild pkgutil shasum spctl xcrun; do
  require_command "$command_name"
done

for binary in sandboxctl agent-client; do
  [ -x "$SOURCE_DIR/$binary" ] || { echo "required executable not found: $SOURCE_DIR/$binary" >&2; exit 1; }
  file "$SOURCE_DIR/$binary" | grep -Eq 'Mach-O.*arm64|Mach-O 64-bit.*arm64' \
    || { echo "required Apple Silicon Mach-O binary not found: $SOURCE_DIR/$binary" >&2; exit 1; }
done

TMP_ROOT="$(mktemp -d -t agentic-macos-package.XXXXXX)"
trap 'rm -rf "$TMP_ROOT"' EXIT

PAYLOAD="$TMP_ROOT/payload"
DMG_ROOT="$TMP_ROOT/dmg-root"
mkdir -p "$PAYLOAD/usr/local/bin" "$PAYLOAD/usr/local/share/doc/agentic-sandbox" "$DMG_ROOT"

install -m 0755 "$SOURCE_DIR/sandboxctl" "$PAYLOAD/usr/local/bin/sandboxctl"
install -m 0755 "$SOURCE_DIR/agent-client" "$PAYLOAD/usr/local/bin/agent-client"
ln -s sandboxctl "$PAYLOAD/usr/local/bin/agentic-sandbox"
install -m 0644 "$ROOT/README.md" "$PAYLOAD/usr/local/share/doc/agentic-sandbox/README.md"
install -m 0644 "$ROOT/LICENSE" "$PAYLOAD/usr/local/share/doc/agentic-sandbox/LICENSE"
install -m 0644 "$ROOT/CHANGELOG.md" "$PAYLOAD/usr/local/share/doc/agentic-sandbox/CHANGELOG.md"

cat > "$PAYLOAD/usr/local/share/doc/agentic-sandbox/MACOS_SCOPE.md" <<'EOF'
# macOS package scope

This preview package installs the Apple Silicon `sandboxctl` and `agent-client`
tools. It does not contain `agentic-mgmt`, a VM provider, launchd services, or
system-wide runtime configuration. macOS management/runtime support remains
gated on the Apple `container` feasibility and provider work in issues #438,
#488, and #489.
EOF

for binary in sandboxctl agent-client; do
  codesign --force --options runtime --timestamp \
    --sign "$APPLE_DEVELOPER_ID_APPLICATION" \
    "$PAYLOAD/usr/local/bin/$binary"
  codesign --verify --strict --verbose=2 "$PAYLOAD/usr/local/bin/$binary"
done

mkdir -p "$OUT_DIR"
BASE="agentic-sandbox-v${VERSION}-aarch64-darwin"
PKG_PATH="$OUT_DIR/${BASE}.pkg"
DMG_PATH="$OUT_DIR/${BASE}.dmg"
SUMS_PATH="$OUT_DIR/SHA256SUMS-macos"
rm -f "$PKG_PATH" "$DMG_PATH" "$SUMS_PATH" "$PKG_PATH.sha256" "$DMG_PATH.sha256"

pkgbuild \
  --root "$PAYLOAD" \
  --identifier net.integrolabs.agentic-sandbox.client-tools \
  --version "$VERSION" \
  --install-location / \
  --sign "$APPLE_DEVELOPER_ID_INSTALLER" \
  "$PKG_PATH"

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

printf 'macOS package verification passed:\n  %s\n  %s\n' "$PKG_PATH" "$DMG_PATH"
