#!/usr/bin/env bash
# Validate public macOS signing selectors and notary profile availability.
# Identity inventory and notary history are consumed in pipelines and discarded.

set -euo pipefail
umask 077

OUTPUT=""
INVENTORY=0

usage() {
  cat <<'EOF'
Usage:
  scripts/macos-release-preflight.sh --output <sanitized.json>
  scripts/macos-release-preflight.sh --inventory --output <sanitized.json>

Required non-secret environment:
  APPLE_DEVELOPER_ID_TEAM_ID
  APPLE_DEVELOPER_ID_APPLICATION
  APPLE_DEVELOPER_ID_INSTALLER
  APPLE_NOTARY_KEYCHAIN_PROFILE

The output contains only the exact public selectors, expected Team ID,
availability/uniqueness state, and profile name. Certificate inventory,
notary history, private key material, and credential contents are never
printed or retained.

Inventory mode discovers only public Developer ID selector names, their
embedded Team IDs, and duplicate counts. It never invokes notarytool or reads
notary-profile state; profile availability is checked only by exact-name
ceremony preflight.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --inventory) INVENTORY=1; shift ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$OUTPUT" ]] || { echo "missing --output" >&2; exit 2; }

require_env() {
  local name="$1"
  [[ -n "${!name:-}" ]] \
    || { printf 'required public selector is not configured: %s\n' "$name" >&2; exit 1; }
}

for command_name in awk jq security; do
  command -v "$command_name" >/dev/null 2>&1 \
    || { printf 'required command not found: %s\n' "$command_name" >&2; exit 1; }
done

if [[ "$INVENTORY" == "1" ]]; then
  scratch="$(mktemp -d -t agentic-macos-public-inventory.XXXXXX)"
  # shellcheck disable=SC2317  # invoked by the EXIT/INT/TERM trap
  cleanup_inventory() {
    rm -rf "$scratch"
  }
  trap cleanup_inventory EXIT INT TERM

  collect_public_selectors() {
    local policy="$1"
    local identity_kind="$2"
    local sanitized_output="$3"
    security find-identity -v -p "$policy" 2>/dev/null |
      awk -F '"' -v prefix="Developer ID ${identity_kind}: " '
        NF >= 3 && index($2, prefix) == 1 &&
          $2 ~ /\([A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9]\)$/ {
            selector = $2
            team_id = substr(selector, length(selector) - 10, 10)
            print selector "\t" team_id
          }
      ' > "$sanitized_output"
  }

  application_public="$scratch/application.tsv"
  installer_public="$scratch/installer.tsv"
  collect_public_selectors codesigning Application "$application_public" \
    || { echo "Application public selector inventory failed" >&2; exit 1; }
  collect_public_selectors basic Installer "$installer_public" \
    || { echo "Installer public selector inventory failed" >&2; exit 1; }

  application_json="$(
    jq -Rn '
      [inputs | split("\t") | {selector:.[0],team_id:.[1]}] |
      group_by([.selector,.team_id]) |
      map({
        selector:.[0].selector,
        team_id:.[0].team_id,
        match_count:length
      })
    ' < "$application_public"
  )"
  installer_json="$(
    jq -Rn '
      [inputs | split("\t") | {selector:.[0],team_id:.[1]}] |
      group_by([.selector,.team_id]) |
      map({
        selector:.[0].selector,
        team_id:.[0].team_id,
        match_count:length
      })
    ' < "$installer_public"
  )"

  jq -n \
    --arg schema_version "agentic.macos-public-identity-inventory.v1" \
    --argjson application_identities "$application_json" \
    --argjson installer_identities "$installer_json" \
    '{
      schema_version:$schema_version,
      application_identities:$application_identities,
      installer_identities:$installer_identities,
      credential_contents_retained:false
    }' > "$OUTPUT"
  chmod 0644 "$OUTPUT"
  exit 0
fi

command -v xcrun >/dev/null 2>&1 \
  || { echo "required command not found: xcrun" >&2; exit 1; }

for name in \
  APPLE_DEVELOPER_ID_TEAM_ID \
  APPLE_DEVELOPER_ID_APPLICATION \
  APPLE_DEVELOPER_ID_INSTALLER \
  APPLE_NOTARY_KEYCHAIN_PROFILE; do
  require_env "$name"
done

[[ "$APPLE_DEVELOPER_ID_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] \
  || { echo "expected Apple Team ID has an invalid public format" >&2; exit 1; }
[[ "$APPLE_NOTARY_KEYCHAIN_PROFILE" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] \
  || { echo "notary profile name has an invalid public format" >&2; exit 1; }
[[ "$APPLE_DEVELOPER_ID_APPLICATION" =~ ^Developer\ ID\ Application:\ .+\ \([A-Z0-9]{10}\)$ ]] \
  || { echo "Application identity selector has an invalid public format" >&2; exit 1; }
[[ "$APPLE_DEVELOPER_ID_INSTALLER" =~ ^Developer\ ID\ Installer:\ .+\ \([A-Z0-9]{10}\)$ ]] \
  || { echo "Installer identity selector has an invalid public format" >&2; exit 1; }

expected_application_suffix=" (${APPLE_DEVELOPER_ID_TEAM_ID})"
[[ "$APPLE_DEVELOPER_ID_APPLICATION" == "Developer ID Application: "*"$expected_application_suffix" ]] \
  || { echo "Application identity selector does not bind the expected Team ID" >&2; exit 1; }
expected_installer_suffix=" (${APPLE_DEVELOPER_ID_TEAM_ID})"
[[ "$APPLE_DEVELOPER_ID_INSTALLER" == "Developer ID Installer: "*"$expected_installer_suffix" ]] \
  || { echo "Installer identity selector does not bind the expected Team ID" >&2; exit 1; }

count_exact_selector() {
  local policy="$1"
  local selector="$2"
  security find-identity -v -p "$policy" 2>/dev/null |
    awk -v quoted="\"${selector}\"" \
      'index($0, quoted) { count++ } END { print count + 0 }'
}

application_count="$(count_exact_selector codesigning "$APPLE_DEVELOPER_ID_APPLICATION")" \
  || { echo "Application identity preflight failed" >&2; exit 1; }
installer_count="$(count_exact_selector basic "$APPLE_DEVELOPER_ID_INSTALLER")" \
  || { echo "Installer identity preflight failed" >&2; exit 1; }
[[ "$application_count" == "1" ]] \
  || { echo "Application identity is unavailable or ambiguous" >&2; exit 1; }
[[ "$installer_count" == "1" ]] \
  || { echo "Installer identity is unavailable or ambiguous" >&2; exit 1; }

if ! xcrun notarytool history \
  --keychain-profile "$APPLE_NOTARY_KEYCHAIN_PROFILE" \
  --output-format json \
  2>/dev/null |
  jq -e 'type == "object" or type == "array"' >/dev/null; then
  echo "notary profile is unavailable" >&2
  exit 1
fi

jq -n \
  --arg schema_version "agentic.macos-release-preflight.v1" \
  --arg team_id "$APPLE_DEVELOPER_ID_TEAM_ID" \
  --arg application_selector "$APPLE_DEVELOPER_ID_APPLICATION" \
  --arg installer_selector "$APPLE_DEVELOPER_ID_INSTALLER" \
  --arg notary_profile "$APPLE_NOTARY_KEYCHAIN_PROFILE" \
  '{
    schema_version: $schema_version,
    team_id: $team_id,
    application_identity: {
      selector: $application_selector,
      team_id: $team_id,
      match_count: 1
    },
    installer_identity: {
      selector: $installer_selector,
      team_id: $team_id,
      match_count: 1
    },
    notary_profile: {
      name: $notary_profile,
      available: true,
      match_count: 1
    },
    credential_contents_retained: false
  }' > "$OUTPUT"
chmod 0644 "$OUTPUT"
