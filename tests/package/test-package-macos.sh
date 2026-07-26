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
if [ "${1:-}" = "-d" ]; then
  if [ "${2:-}" = "--entitlements" ]; then
    printf 'SYNTHETIC_ENTITLEMENTS\n'
  else
    {
      printf 'Authority=Developer ID Application: Synthetic Fixture (SYNTH12345)\n'
      printf 'TeamIdentifier=SYNTH12345\n'
      printf 'Timestamp=Jul 25, 2026 at 12:00:00\n'
    } >&2
  fi
  exit 0
fi
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

cat > "$FAKE_BIN/pkgutil" <<'EOF'
#!/usr/bin/env bash
printf '%s %s\n' "$(basename "$0")" "$*" >> "$AGENTIC_TEST_COMMAND_LOG"
if [ "${1:-}" = "--expand" ]; then
  mkdir -p "$3"
  cat > "$3/PackageInfo" <<XML
<?xml version="1.0"?>
<pkg-info identifier="io.aiwg.agentic-sandbox" version="2026.7.13"/>
XML
fi
if [ "${1:-}" = "--check-signature" ]; then
  printf 'Status: signed by a certificate trusted by macOS\n'
  printf 'Developer ID Installer: Synthetic Fixture (SYNTH12345)\n'
fi
EOF

cat > "$FAKE_BIN/spctl" <<'EOF'
#!/usr/bin/env bash
printf '%s %s\n' "$(basename "$0")" "$*" >> "$AGENTIC_TEST_COMMAND_LOG"
EOF

cat > "$FAKE_BIN/xcrun" <<'EOF'
#!/usr/bin/env bash
printf '%s %s\n' "$(basename "$0")" "$*" >> "$AGENTIC_TEST_COMMAND_LOG"
if [ "${1:-}" = "notarytool" ] && [ "${2:-}" = "history" ]; then
  printf '{}\n'
fi
if [ "${AGENTIC_TEST_FAIL_NOTARY_SUBMIT:-0}" = "1" ] &&
   [ "${1:-}" = "notarytool" ] && [ "${2:-}" = "submit" ]; then
  exit 1
fi
EOF

cat > "$FAKE_BIN/security" <<'EOF'
#!/usr/bin/env bash
if [ "${AGENTIC_TEST_IDENTITY_STATE:-unique}" != "missing" ]; then
  printf '  1) AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA "Developer ID Application: Synthetic Fixture (SYNTH12345)"\n'
fi
printf '  2) BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB "Developer ID Installer: Synthetic Fixture (SYNTH12345)"\n'
if [ "${AGENTIC_TEST_IDENTITY_STATE:-unique}" = "ambiguous" ]; then
  printf '  3) CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC "Developer ID Application: Synthetic Fixture (SYNTH12345)"\n'
fi
EOF

cat > "$FAKE_BIN/plutil" <<'EOF'
#!/usr/bin/env bash
for arg in "$@"; do input="$arg"; done
if grep -q SYNTHETIC_ENTITLEMENTS "$input" 2>/dev/null; then
  if [ -n "${AGENTIC_TEST_ACTUAL_ENTITLEMENTS_JSON:-}" ]; then
    printf '%s\n' "$AGENTIC_TEST_ACTUAL_ENTITLEMENTS_JSON"
  else
    printf '{}\n'
  fi
else
  printf '{}\n'
fi
EOF

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
    # Positional parameters expand safely to zero words under macOS Bash 3.2
    # with `set -u`; an empty array expansion does not.
    set --
    if [ -n "$approved_manifest" ]; then
      set -- \
        --approved-payload-manifest "$approved_manifest" \
        --approved-preview-package \
          "$APPROVAL_OUT/agentic-sandbox-v2026.7.13-aarch64-darwin-preview.pkg" \
        --operator-approval-ref "issue-677/synthetic-witness" \
        --source-commit "1111111111111111111111111111111111111111" \
        --release-tag "v2026.7.13"
    fi
    PATH="$FAKE_BIN:$PATH" \
      CI=true \
      AGENTIC_MACOS_PACKAGING_TEST_MODE=1 \
      AGENTIC_TEST_COMMAND_LOG="$LOG" \
      AGENTIC_TEST_PAYLOAD_SNAPSHOT="$snapshot" \
      APPLE_DEVELOPER_ID_TEAM_ID='SYNTH12345' \
      APPLE_DEVELOPER_ID_APPLICATION='Developer ID Application: Synthetic Fixture (SYNTH12345)' \
      APPLE_DEVELOPER_ID_INSTALLER='Developer ID Installer: Synthetic Fixture (SYNTH12345)' \
      APPLE_NOTARY_KEYCHAIN_PROFILE='synthetic-notary-profile' \
      scripts/package-macos.sh \
        --mode "$mode" \
        --version v2026.7.13 \
        --source-dir "$SOURCE_DIR" \
        --out-dir "$output" \
        "$@"
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
  .schema_version == "agentic.macos-release-evidence.v1" and
  .package.identifier == "io.aiwg.agentic-sandbox" and
  .package.team_id == "SYNTH12345" and
  .release.source_commit == "1111111111111111111111111111111111111111" and
  .release.tag == "v2026.7.13" and
  .release.operator_approval_ref == "issue-677/synthetic-witness" and
  .credential_contents_retained == false and
  (.payloads | length == 4) and
  (.payloads | all(.entitlements_exact_match == true)) and
  (.artifacts | length == 2) and
  (.artifacts | all(.sha256 == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
' "$EVIDENCE" >/dev/null || fail "release evidence is incomplete or secret-bearing"
"$ROOT/scripts/validate-macos-release-evidence.py" "$EVIDENCE" >/dev/null \
  || fail "closed release evidence validator rejected production evidence"
pass "production artifact set is deterministic"

grep -F -- '--options runtime --timestamp --entitlements' "$LOG" >/dev/null \
  || fail "Mach-O binaries were not hardened-runtime signed"
test "$(grep -c '^codesign --force --options runtime' "$LOG")" -eq 4 \
  || fail "all four Apple runtime binaries were not signed"
grep -F -- '--sign Developer ID Installer: Synthetic Fixture (SYNTH12345)' "$LOG" >/dev/null \
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
    APPLE_DEVELOPER_ID_TEAM_ID='SYNTH12345' \
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

PREFLIGHT_OUT="$TMP/preflight.json"
PATH="$FAKE_BIN:$PATH" \
  APPLE_DEVELOPER_ID_TEAM_ID='SYNTH12345' \
  APPLE_DEVELOPER_ID_APPLICATION='Developer ID Application: Synthetic Fixture (SYNTH12345)' \
  APPLE_DEVELOPER_ID_INSTALLER='Developer ID Installer: Synthetic Fixture (SYNTH12345)' \
  APPLE_NOTARY_KEYCHAIN_PROFILE='synthetic-notary-profile' \
  scripts/macos-release-preflight.sh --output "$PREFLIGHT_OUT"
jq -e '
  .schema_version == "agentic.macos-release-preflight.v1" and
  .team_id == "SYNTH12345" and
  .application_identity.match_count == 1 and
  .installer_identity.match_count == 1 and
  .notary_profile.available == true and
  .notary_profile.match_count == 1 and
  .credential_contents_retained == false
' "$PREFLIGHT_OUT" >/dev/null || fail "sanitized preflight evidence is incomplete"
pass "preflight retains only sanitized public selector metadata"

INVENTORY_OUT="$TMP/public-inventory.json"
INVENTORY_COMMAND_LOG="$TMP/public-inventory-commands.log"
: > "$INVENTORY_COMMAND_LOG"
PATH="$FAKE_BIN:$PATH" \
  AGENTIC_TEST_COMMAND_LOG="$INVENTORY_COMMAND_LOG" \
  scripts/macos-release-preflight.sh --inventory --output "$INVENTORY_OUT"
jq -e '
  .schema_version == "agentic.macos-public-identity-inventory.v1" and
  .application_identities == [{
    selector:"Developer ID Application: Synthetic Fixture (SYNTH12345)",
    team_id:"SYNTH12345",
    match_count:1
  }] and
  .installer_identities == [{
    selector:"Developer ID Installer: Synthetic Fixture (SYNTH12345)",
    team_id:"SYNTH12345",
    match_count:1
  }] and
  (has("notary_profile") | not) and
  .credential_contents_retained == false
' "$INVENTORY_OUT" >/dev/null || fail "public identity inventory is incomplete"
if grep -Eq 'A{40}|B{40}|C{40}' "$INVENTORY_OUT"; then
  fail "public identity inventory retained certificate hashes"
fi
if grep -Fq 'notarytool' "$INVENTORY_COMMAND_LOG"; then
  fail "public identity inventory invoked notarytool"
fi
pass "inventory discovers only public selectors, Team IDs, and counts"

for state in missing ambiguous; do
  PREFLIGHT_ERROR="$TMP/preflight-${state}.error"
  if PATH="$FAKE_BIN:$PATH" \
    AGENTIC_TEST_IDENTITY_STATE="$state" \
    APPLE_DEVELOPER_ID_TEAM_ID='SYNTH12345' \
    APPLE_DEVELOPER_ID_APPLICATION='Developer ID Application: Synthetic Fixture (SYNTH12345)' \
    APPLE_DEVELOPER_ID_INSTALLER='Developer ID Installer: Synthetic Fixture (SYNTH12345)' \
    APPLE_NOTARY_KEYCHAIN_PROFILE='synthetic-notary-profile' \
    scripts/macos-release-preflight.sh --output "$TMP/preflight-${state}.json" \
    >"$TMP/preflight-${state}.stdout" 2>"$PREFLIGHT_ERROR"; then
    fail "preflight accepted an ${state} Application identity"
  fi
  if grep -Eq '[A-F0-9]{40}|private|password|secret' \
    "$TMP/preflight-${state}.stdout" "$PREFLIGHT_ERROR"; then
    fail "preflight failure exposed identity inventory or secret-bearing metadata"
  fi
done
pass "missing and ambiguous identities fail closed without inventory disclosure"

if PATH="$FAKE_BIN:$PATH" \
  APPLE_DEVELOPER_ID_TEAM_ID='OTHER12345' \
  APPLE_DEVELOPER_ID_APPLICATION='Developer ID Application: Synthetic Fixture (SYNTH12345)' \
  APPLE_DEVELOPER_ID_INSTALLER='Developer ID Installer: Synthetic Fixture (SYNTH12345)' \
  APPLE_NOTARY_KEYCHAIN_PROFILE='synthetic-notary-profile' \
  scripts/macos-release-preflight.sh --output "$TMP/team-mismatch.json" \
  >/dev/null 2>&1; then
  fail "preflight accepted identity selectors for another Team ID"
fi
pass "identity selectors bind the exact expected Team ID"

if PATH="$FAKE_BIN:$PATH" \
  AGENTIC_TEST_ACTUAL_ENTITLEMENTS_JSON='{"com.apple.security.get-task-allow":true}' \
  scripts/verify-macos-entitlements.sh \
    --expected deploy/packaging/macos/agentic-sandbox.entitlements.plist \
    --output "$TMP/extra-entitlements.tsv" \
    "$SOURCE_DIR/agentic-mgmt" >/dev/null 2>&1; then
  fail "entitlement verifier accepted an undeclared entitlement"
fi
pass "no-extra-entitlements policy rejects any entitlement drift"

SECRET_VALUE='synthetic-value-must-not-appear'
SECRET_EVIDENCE="$TMP/secret-bearing-evidence.json"
jq --arg value "$SECRET_VALUE" '.private_key = $value' "$EVIDENCE" > "$SECRET_EVIDENCE"
if "$ROOT/scripts/validate-macos-release-evidence.py" "$SECRET_EVIDENCE" \
  >"$TMP/secret-validator.stdout" 2>"$TMP/secret-validator.stderr"; then
  fail "closed evidence validator accepted a private-key field"
fi
if grep -Fq "$SECRET_VALUE" "$TMP/secret-validator.stdout" "$TMP/secret-validator.stderr"; then
  fail "closed evidence validator echoed a rejected field value"
fi

EXTRA_EVIDENCE="$TMP/extra-evidence.json"
jq '.release.unexpected = "synthetic"' "$EVIDENCE" > "$EXTRA_EVIDENCE"
if "$ROOT/scripts/validate-macos-release-evidence.py" "$EXTRA_EVIDENCE" \
  >/dev/null 2>&1; then
  fail "closed evidence validator accepted an unspecified field"
fi

RETAINED_EVIDENCE="$TMP/retained-evidence.json"
jq '.credential_contents_retained = true' "$EVIDENCE" > "$RETAINED_EVIDENCE"
if "$ROOT/scripts/validate-macos-release-evidence.py" "$RETAINED_EVIDENCE" \
  >/dev/null 2>&1; then
  fail "closed evidence validator accepted retained credential contents"
fi

if "$ROOT/scripts/validate-macos-release-evidence.py" "$EVIDENCE" \
  --expect-source-commit 2222222222222222222222222222222222222222 \
  >/dev/null 2>&1; then
  fail "release evidence validator accepted a source-commit binding mismatch"
fi
RELATION_EVIDENCE="$TMP/relation-evidence.json"
jq '.release.tag = "v2026.7.14"' "$EVIDENCE" > "$RELATION_EVIDENCE"
if "$ROOT/scripts/validate-macos-release-evidence.py" "$RELATION_EVIDENCE" \
  >/dev/null 2>&1; then
  fail "release evidence validator accepted inconsistent version and tag"
fi
printf '{"schema_version":"one","schema_version":"two"}\n' \
  > "$TMP/duplicate-fields.json"
if "$ROOT/scripts/validate-macos-release-evidence.py" "$TMP/duplicate-fields.json" \
  >/dev/null 2>&1; then
  fail "release evidence validator accepted duplicate JSON fields"
fi
pass "closed evidence schema rejects secret-bearing, extra, retained, and mismatched data"

HANDOFF_BUILD="$TMP/handoff-build"
HANDOFF_PARENT="$TMP/handoff-parent"
mkdir -p "$HANDOFF_BUILD" "$HANDOFF_PARENT"
BASE_NAME="agentic-sandbox-v2026.7.13-aarch64-darwin"
cp "$PKG" "$HANDOFF_BUILD/$BASE_NAME.pkg"
cp "$DMG" "$HANDOFF_BUILD/$BASE_NAME.dmg"
cp "$PRODUCTION_OUT/$BASE_NAME.payload-manifest.tsv" \
  "$HANDOFF_BUILD/$BASE_NAME.payload-manifest.tsv"
PKG_REAL_SHA="$(shasum -a 256 "$HANDOFF_BUILD/$BASE_NAME.pkg" | awk '{print $1}')"
DMG_REAL_SHA="$(shasum -a 256 "$HANDOFF_BUILD/$BASE_NAME.dmg" | awk '{print $1}')"
MANIFEST_REAL_SHA="$(
  shasum -a 256 "$HANDOFF_BUILD/$BASE_NAME.payload-manifest.tsv" | awk '{print $1}'
)"
(
  cd "$HANDOFF_BUILD"
  printf '%s  %s\n' "$PKG_REAL_SHA" "$BASE_NAME.pkg" > "$BASE_NAME.pkg.sha256"
  printf '%s  %s\n' "$DMG_REAL_SHA" "$BASE_NAME.dmg" > "$BASE_NAME.dmg.sha256"
  {
    printf '%s  %s\n' "$PKG_REAL_SHA" "$BASE_NAME.pkg"
    printf '%s  %s\n' "$DMG_REAL_SHA" "$BASE_NAME.dmg"
  } > SHA256SUMS-macos
)
jq \
  --arg pkg_sha "$PKG_REAL_SHA" \
  --arg dmg_sha "$DMG_REAL_SHA" \
  --arg manifest_sha "$MANIFEST_REAL_SHA" '
    (.artifacts[] | select(.kind == "pkg") | .sha256) = $pkg_sha |
    (.artifacts[] | select(.kind == "dmg") | .sha256) = $dmg_sha |
    .package.signed_payload_manifest.sha256 = $manifest_sha
  ' "$EVIDENCE" > "$HANDOFF_BUILD/$BASE_NAME.release-evidence.json"
EVIDENCE_REAL_SHA="$(
  shasum -a 256 "$HANDOFF_BUILD/$BASE_NAME.release-evidence.json" | awk '{print $1}'
)"
HANDOFF_DIR="$HANDOFF_PARENT/$EVIDENCE_REAL_SHA"
mkdir "$HANDOFF_DIR"
cp "$HANDOFF_BUILD"/* "$HANDOFF_DIR/"
jq -n \
  --arg release_tag v2026.7.13 \
  --arg source_commit 1111111111111111111111111111111111111111 \
  --arg release_evidence_sha256 "$EVIDENCE_REAL_SHA" '
    {
      schema_version:"agentic.macos-release-handoff.v1",
      release_tag:$release_tag,
      source_commit:$source_commit,
      release_evidence_sha256:$release_evidence_sha256,
      immutable:true,
      credential_contents_retained:false
    }
  ' > "$HANDOFF_DIR/handoff.json"
"$ROOT/scripts/verify-macos-release-handoff.sh" \
  --handoff-dir "$HANDOFF_DIR" \
  --expected-evidence-sha256 "$EVIDENCE_REAL_SHA" \
  --source-commit 1111111111111111111111111111111111111111 \
  --release-tag v2026.7.13 \
  --output-dir "$TMP/promoted" >/dev/null \
  || fail "immutable handoff verifier rejected exact ceremony bytes"
test -f "$TMP/promoted/$BASE_NAME.pkg" \
  || fail "immutable handoff verifier did not stage the exact package"

if "$ROOT/scripts/verify-macos-release-handoff.sh" \
  --handoff-dir "$HANDOFF_DIR" \
  --expected-evidence-sha256 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
  --source-commit 1111111111111111111111111111111111111111 \
  --release-tag v2026.7.13 \
  --output-dir "$TMP/unapproved-promotion" >/dev/null 2>&1; then
  fail "immutable handoff verifier accepted an unapproved evidence digest"
fi

printf 'tamper\n' >> "$HANDOFF_DIR/$BASE_NAME.pkg"
if "$ROOT/scripts/verify-macos-release-handoff.sh" \
  --handoff-dir "$HANDOFF_DIR" \
  --expected-evidence-sha256 "$EVIDENCE_REAL_SHA" \
  --source-commit 1111111111111111111111111111111111111111 \
  --release-tag v2026.7.13 \
  --output-dir "$TMP/tampered-promotion" >/dev/null 2>&1; then
  fail "immutable handoff verifier accepted artifact digest drift"
fi
pass "tag promotion consumes only exact evidence-bound Apple bytes"

ABORT_OUT="$OUT_DIR/abort"
mkdir -p "$ABORT_OUT"
if AGENTIC_TEST_FAIL_NOTARY_SUBMIT=1 \
  run_packager production "$ABORT_OUT" "$TMP/abort-payload" "$APPROVED_MANIFEST" \
  >/dev/null 2>&1; then
  fail "production ceremony continued after synthetic notarization failure"
fi
test ! -e "$ABORT_OUT/agentic-sandbox-v2026.7.13-aarch64-darwin.pkg" \
  || fail "aborted production artifact remained eligible for publication"
test -f "$ABORT_OUT/quarantine/agentic-sandbox-v2026.7.13-aarch64-darwin.pkg" \
  || fail "aborted production package was not quarantined"
jq -e '
  .state == "quarantined" and
  .source_commit == "1111111111111111111111111111111111111111" and
  .credential_contents_retained == false
' "$ABORT_OUT/quarantine/quarantine.json" >/dev/null \
  || fail "quarantine evidence is missing sanitized release bindings"
pass "synthetic ceremony abort quarantines every partial publication artifact"
