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

for binary in agentic-mgmt agentic-host-runtime-daemon sandboxctl agent-client; do
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
root=""
previous=""
for arg in "$@"; do
  if [ "$previous" = "--root" ]; then root="$arg"; fi
  previous="$arg"
  output="$arg"
done
if [ -n "${AGENTIC_TEST_PAYLOAD_SNAPSHOT:-}" ]; then
  rm -rf "$AGENTIC_TEST_PAYLOAD_SNAPSHOT"
  cp -a "$root" "$AGENTIC_TEST_PAYLOAD_SNAPSHOT"
fi
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
  mode="$1"
  output="$2"
  snapshot="$3"
  approved_manifest="${4:-}"
  (
    cd "$ROOT"
    approved_args=()
    if [ -n "$approved_manifest" ]; then
      approved_args=(--approved-payload-manifest "$approved_manifest")
    fi
    PATH="$FAKE_BIN:$PATH" \
      CI=true \
      AGENTIC_MACOS_PACKAGING_TEST_MODE=1 \
      AGENTIC_TEST_COMMAND_LOG="$LOG" \
      AGENTIC_TEST_PAYLOAD_SNAPSHOT="$snapshot" \
      APPLE_DEVELOPER_ID_APPLICATION='Developer ID Application: Synthetic Fixture' \
      APPLE_DEVELOPER_ID_INSTALLER='Developer ID Installer: Synthetic Fixture' \
      APPLE_NOTARY_KEYCHAIN_PROFILE='synthetic-notary-profile' \
      scripts/package-macos.sh \
        --mode "$mode" \
        --version v2026.7.13 \
        --source-dir "$SOURCE_DIR" \
        --out-dir "$output" \
        "${approved_args[@]}"
  )
}

APPROVAL_OUT="$OUT_DIR/approval"
APPROVAL_PAYLOAD="$TMP/approval-payload"
mkdir -p "$APPROVAL_OUT"
run_packager preview "$APPROVAL_OUT" "$APPROVAL_PAYLOAD" >/dev/null
APPROVED_MANIFEST="$APPROVAL_OUT/agentic-sandbox-v2026.7.13-aarch64-darwin.payload-manifest.tsv"

PRODUCTION_OUT="$OUT_DIR/production"
PRODUCTION_PAYLOAD="$TMP/production-payload"
mkdir -p "$PRODUCTION_OUT"
run_packager production "$PRODUCTION_OUT" "$PRODUCTION_PAYLOAD" "$APPROVED_MANIFEST" >/dev/null

PKG="$PRODUCTION_OUT/agentic-sandbox-v2026.7.13-aarch64-darwin.pkg"
DMG="$PRODUCTION_OUT/agentic-sandbox-v2026.7.13-aarch64-darwin.dmg"
test -f "$PKG" || fail "production pkg was not produced"
test -f "$DMG" || fail "production dmg was not produced"
test -f "$PRODUCTION_OUT/SHA256SUMS-macos" || fail "macOS checksum manifest was not produced"
test -f "$PKG.sha256" || fail "pkg checksum sidecar was not produced"
test -f "$DMG.sha256" || fail "dmg checksum sidecar was not produced"
test -f "$PRODUCTION_OUT/agentic-sandbox-v2026.7.13-aarch64-darwin.payload-manifest.tsv" \
  || fail "payload manifest sidecar was not produced"
EVIDENCE="$PRODUCTION_OUT/agentic-sandbox-v2026.7.13-aarch64-darwin.release-evidence.json"
test -f "$EVIDENCE" || fail "sanitized release evidence was not produced"
jq -e '
  .package_identifier == "io.aiwg.agentic-sandbox" and
  .credential_contents_retained == false and
  (.artifacts | length == 2) and
  (.artifacts | all(.sha256 == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
' "$EVIDENCE" >/dev/null || fail "release evidence is incomplete or secret-bearing"
pass "production artifact set is deterministic"

grep -F -- '--options runtime --timestamp --sign Developer ID Application: Synthetic Fixture' "$LOG" >/dev/null \
  || fail "Mach-O binaries were not hardened-runtime signed"
test "$(grep -c '^codesign --force --options runtime' "$LOG")" -eq 4 \
  || fail "all four Apple runtime binaries were not signed"
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

for binary in agentic-mgmt agentic-host-runtime-daemon sandboxctl agent-client; do
  test -x "$PRODUCTION_PAYLOAD/usr/local/bin/$binary" \
    || fail "full Apple runtime payload omitted $binary"
done
test -L "$PRODUCTION_PAYLOAD/usr/local/bin/agentic-sandbox" \
  || fail "CLI compatibility symlink was omitted"
test -x "$PRODUCTION_PAYLOAD/usr/local/libexec/agentic-sandbox/render-macos-launch-agent" \
  || fail "LaunchAgent renderer was omitted"
test -x "$PRODUCTION_PAYLOAD/usr/local/libexec/agentic-sandbox/uninstall-macos" \
  || fail "bounded uninstaller was omitted"
test -f "$PRODUCTION_PAYLOAD/usr/local/share/agentic-sandbox/launchd/io.aiwg.agentic-sandbox.host-runtime.plist" \
  || fail "inert LaunchAgent template was omitted"
test -f "$PRODUCTION_PAYLOAD/usr/local/share/agentic-sandbox/config/agentic-sandbox.env.example" \
  || fail "macOS configuration example was omitted"
PAYLOAD_MANIFEST="$PRODUCTION_PAYLOAD/usr/local/share/doc/agentic-sandbox/PAYLOAD-MANIFEST.tsv"
test -s "$PAYLOAD_MANIFEST" || fail "payload manifest was omitted"
grep -F $'file\t755\tusr/local/bin/agentic-mgmt\t' "$PAYLOAD_MANIFEST" >/dev/null \
  || fail "payload manifest does not pin management mode/digest"
grep -F $'symlink\t-\tusr/local/bin/agentic-sandbox\tsandboxctl' "$PAYLOAD_MANIFEST" >/dev/null \
  || fail "payload manifest does not pin the CLI symlink"
pass "full runtime payload, permissions, and launchd/config assets are deterministic"

PREVIEW_OUT="$OUT_DIR/preview"
PREVIEW_PAYLOAD="$TMP/preview-payload"
PREVIEW_LOG="$TMP/preview-commands.log"
mkdir -p "$PREVIEW_OUT"
(
  cd "$ROOT"
  PATH="$FAKE_BIN:$PATH" \
    CI=true \
    AGENTIC_MACOS_PACKAGING_TEST_MODE=1 \
    AGENTIC_TEST_COMMAND_LOG="$PREVIEW_LOG" \
    AGENTIC_TEST_PAYLOAD_SNAPSHOT="$PREVIEW_PAYLOAD" \
    scripts/package-macos.sh \
      --mode preview \
      --version v2026.7.13 \
      --source-dir "$SOURCE_DIR" \
      --out-dir "$PREVIEW_OUT"
) >/dev/null
PREVIEW_PKG="$PREVIEW_OUT/agentic-sandbox-v2026.7.13-aarch64-darwin-preview.pkg"
test -f "$PREVIEW_PKG" || fail "credential-free preview pkg was not produced"
test -f "$PREVIEW_PKG.sha256" || fail "preview pkg checksum was not produced"
test -f "$PREVIEW_OUT/agentic-sandbox-v2026.7.13-aarch64-darwin.payload-manifest.tsv" \
  || fail "preview payload manifest sidecar was not produced"
if grep -Eq 'codesign|notarytool|stapler|spctl' "$PREVIEW_LOG"; then
  fail "credential-free preview invoked a production trust-chain command"
fi
pass "preview construction requires no signing or notarization identity"

ISOLATED_ROOT="$TMP/isolated-install-root"
cp -a "$PREVIEW_PAYLOAD" "$ISOLATED_ROOT"
"$ISOLATED_ROOT/usr/local/libexec/agentic-sandbox/uninstall-macos" \
  --root "$ISOLATED_ROOT" \
  --manifest /usr/local/share/doc/agentic-sandbox/PAYLOAD-MANIFEST.tsv \
  >/dev/null
for binary in agentic-mgmt agentic-host-runtime-daemon sandboxctl agent-client; do
  test ! -e "$ISOLATED_ROOT/usr/local/bin/$binary" \
    || fail "isolated uninstall left package-owned binary $binary"
done
test ! -e "$ISOLATED_ROOT/usr/local/share/doc/agentic-sandbox/PAYLOAD-MANIFEST.tsv" \
  || fail "isolated uninstall left the package manifest"
pass "isolated-root uninstall removes only package-owned payload paths"

if (
  cd "$ROOT"
  PATH="$FAKE_BIN:$PATH" CI=true AGENTIC_MACOS_PACKAGING_TEST_MODE=1 \
    APPLE_DEVELOPER_ID_APPLICATION='Developer ID Application: Synthetic Fixture' \
    APPLE_DEVELOPER_ID_INSTALLER='Developer ID Installer: Synthetic Fixture' \
    APPLE_NOTARY_KEYCHAIN_PROFILE='synthetic-notary-profile' \
    scripts/package-macos.sh --mode production --version v2026.7.13 \
      --source-dir "$SOURCE_DIR" --out-dir "$OUT_DIR/missing-approval"
) >/dev/null 2>&1; then
  fail "production packager accepted a missing approved payload manifest"
fi
pass "production promotion requires an approved credential-free manifest"

DRIFTED_MANIFEST="$TMP/drifted-manifest.tsv"
cp "$APPROVED_MANIFEST" "$DRIFTED_MANIFEST"
printf 'file\t644\tunapproved/path\tbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n' \
  >> "$DRIFTED_MANIFEST"
if run_packager production "$OUT_DIR/drifted" "$TMP/drifted-payload" "$DRIFTED_MANIFEST" \
  >/dev/null 2>&1; then
  fail "production packager accepted payload drift from the approved preview"
fi
pass "production promotion rejects payload drift"

if (
  cd "$ROOT"
  PATH="$FAKE_BIN:$PATH" CI=true AGENTIC_MACOS_PACKAGING_TEST_MODE=1 \
    scripts/package-macos.sh --mode production --version v2026.7.13 \
      --source-dir "$SOURCE_DIR" --out-dir "$OUT_DIR/missing-env" \
      --approved-payload-manifest "$APPROVED_MANIFEST"
) >/dev/null 2>&1; then
  fail "packager accepted missing production signing configuration"
fi
pass "missing production signing configuration is rejected"
