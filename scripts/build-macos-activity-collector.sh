#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT="${1:-$ROOT/dist/macos/agentic-macos-activity-collector}"
SDK_PATH="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "macOS activity collector builds require an Apple Silicon macOS host" >&2
  exit 1
fi
if [ -z "$SDK_PATH" ] || [ ! -d "$SDK_PATH/System/Library/Frameworks/EndpointSecurity.framework" ]; then
  echo "the full Xcode macOS SDK with EndpointSecurity.framework is required" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"
xcrun --sdk macosx swiftc -O \
  -framework CryptoKit \
  -framework EndpointSecurity \
  "$ROOT/native/macos-activity-collector/main.swift" \
  -o "$OUTPUT"
file "$OUTPUT" | grep -Eq 'Mach-O.*arm64|Mach-O 64-bit.*arm64' || {
  echo "collector build did not produce an Apple Silicon Mach-O binary" >&2
  exit 1
}
chmod 0755 "$OUTPUT"
printf 'built unsigned collector: %s\n' "$OUTPUT"
printf 'signing must use native/macos-activity-collector/EndpointSecurity.entitlements.plist\n'
