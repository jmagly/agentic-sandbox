#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d -t agentic-macos-package-tests.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

FAKE_BIN="$TMP/bin"
SOURCE_DIR="$TMP/source"
OUT_DIR="$TMP/out"
LOG="$TMP/commands.log"
mkdir -p "$FAKE_BIN" "$SOURCE_DIR" "$OUT_DIR"

fail() { printf 'not ok - %s\n' "$*" >&2; exit 1; }
pass() { printf 'ok - %s\n' "$*"; }

for binary in sandboxctl agent-client; do
  printf '#!/usr/bin/env bash\nexit 0\n' > "$SOURCE_DIR/$binary"
  chmod +x "$SOURCE_DIR/$binary"
done

cat > "$FAKE_BIN/file" <<'EOF'
#!/usr/bin/env bash
printf '%s: Mach-O 64-bit executable arm64\n' "$1"
EOF

cat > "$FAKE_BIN/codesign" <<'EOF'
#!/usr/bin/env bash
printf 'codesign %s\n' "$*" >> "$AGENTIC_TEST_COMMAND_LOG"
EOF

cat > "$FAKE_BIN/pkgbuild" <<'EOF'
#!/usr/bin/env bash
printf 'pkgbuild %s\n' "$*" >> "$AGENTIC_TEST_COMMAND_LOG"
for arg in "$@"; do output="$arg"; done
printf 'synthetic signed pkg\n' > "$output"
EOF

cat > "$FAKE_BIN/hdiutil" <<'EOF'
#!/usr/bin/env bash
printf 'hdiutil %s\n' "$*" >> "$AGENTIC_TEST_COMMAND_LOG"
for arg in "$@"; do output="$arg"; done
printf 'synthetic signed dmg\n' > "$output"
EOF

for command_name in pkgutil spctl xcrun; do
  cat > "$FAKE_BIN/$command_name" <<'EOF'
#!/usr/bin/env bash
printf '%s %s\n' "$(basename "$0")" "$*" >> "$AGENTIC_TEST_COMMAND_LOG"
EOF
done

cat > "$FAKE_BIN/shasum" <<'EOF'
#!/usr/bin/env bash
shift 2
for path in "$@"; do
  printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  %s\n' "$path"
done
EOF

chmod +x "$FAKE_BIN"/*

run_packager() {
  (
    cd "$ROOT"
    PATH="$FAKE_BIN:$PATH" \
      CI=true \
      AGENTIC_MACOS_PACKAGING_TEST_MODE=1 \
      AGENTIC_TEST_COMMAND_LOG="$LOG" \
      APPLE_DEVELOPER_ID_APPLICATION='Developer ID Application: Synthetic Fixture' \
      APPLE_DEVELOPER_ID_INSTALLER='Developer ID Installer: Synthetic Fixture' \
      APPLE_NOTARY_KEYCHAIN_PROFILE='synthetic-notary-profile' \
      scripts/package-macos.sh \
        --version v2026.7.13 \
        --source-dir "$SOURCE_DIR" \
        --out-dir "$OUT_DIR"
  )
}

run_packager >/dev/null

PKG="$OUT_DIR/agentic-sandbox-v2026.7.13-aarch64-darwin.pkg"
DMG="$OUT_DIR/agentic-sandbox-v2026.7.13-aarch64-darwin.dmg"
test -f "$PKG" || fail "signed pkg was not produced"
test -f "$DMG" || fail "signed dmg was not produced"
test -f "$OUT_DIR/SHA256SUMS-macos" || fail "macOS checksum manifest was not produced"
test -f "$PKG.sha256" || fail "pkg checksum sidecar was not produced"
test -f "$DMG.sha256" || fail "dmg checksum sidecar was not produced"
pass "production artifact set is deterministic"

grep -F -- '--options runtime --timestamp --sign Developer ID Application: Synthetic Fixture' "$LOG" >/dev/null \
  || fail "Mach-O binaries were not hardened-runtime signed"
grep -F -- '--sign Developer ID Installer: Synthetic Fixture' "$LOG" >/dev/null \
  || fail "installer was not Developer ID Installer signed"
test "$(grep -c 'notarytool submit' "$LOG")" -eq 2 \
  || fail "pkg and dmg were not both submitted for notarization"
test "$(grep -c 'stapler staple' "$LOG")" -eq 2 \
  || fail "pkg and dmg were not both stapled"
grep -F 'spctl --assess --type install' "$LOG" >/dev/null \
  || fail "installer Gatekeeper assessment was not run"
grep -F 'spctl --assess --type open --context context:primary-signature' "$LOG" >/dev/null \
  || fail "DMG Gatekeeper assessment was not run"
pass "signing, notarization, stapling, and Gatekeeper checks fail closed"

if (
  cd "$ROOT"
  PATH="$FAKE_BIN:$PATH" CI=true AGENTIC_MACOS_PACKAGING_TEST_MODE=1 \
    scripts/package-macos.sh --version v2026.7.13 --source-dir "$SOURCE_DIR" --out-dir "$OUT_DIR/missing-env"
) >/dev/null 2>&1; then
  fail "packager accepted missing production signing configuration"
fi
pass "missing production signing configuration is rejected"
