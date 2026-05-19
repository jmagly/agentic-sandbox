#!/bin/bash
# pin-iso.sh - Populate iso-pins.json sha256 by gpg-verifying SHA256SUMS (#258)
#
# Usage:  pin-iso.sh <version>     # e.g., pin-iso.sh 24.04
#
# Procedure:
#   1. Fetch SHA256SUMS + SHA256SUMS.gpg from releases.ubuntu.com
#   2. Import Ubuntu archive GPG key (sha256 fingerprint pinned in iso-pins.json)
#   3. gpg --verify SHA256SUMS.gpg against SHA256SUMS
#   4. Extract sha256 for live-server-amd64.iso of the point release
#   5. Write back into iso-pins.json with timestamp
#
# Operator runs this once per Ubuntu point release; commit the resulting diff.

set -euo pipefail

VERSION="${1:?usage: pin-iso.sh <version>}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PINS_FILE="${ISO_PINS_FILE:-$SCRIPT_DIR/../iso-pins.json}"

log() { echo "[pin-iso] $*" >&2; }

if [[ ! -f "$PINS_FILE" ]]; then
    log "FATAL: iso-pins.json not found at $PINS_FILE"
    exit 1
fi

for cmd in jq curl gpg sha256sum; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        log "FATAL: $cmd required"
        exit 1
    fi
done

# Read pin block for this version
point=$(jq -r ".releases[\"$VERSION\"].point_release // \"\"" "$PINS_FILE")
sums_url=$(jq -r ".releases[\"$VERSION\"].sha256sums_url // \"\"" "$PINS_FILE")
gpg_url=$(jq -r ".releases[\"$VERSION\"].sha256sums_gpg_url // \"\"" "$PINS_FILE")
fp=$(jq -r ".ubuntu_archive_gpg_fingerprint // \"\"" "$PINS_FILE")
filename_tmpl=$(jq -r ".releases[\"$VERSION\"].iso_filename_pattern // \"\"" "$PINS_FILE")

if [[ -z "$point" || -z "$sums_url" || -z "$gpg_url" || -z "$fp" ]]; then
    log "FATAL: incomplete pin metadata for $VERSION"
    exit 1
fi

iso_filename="${filename_tmpl//\$\{point_release\}/$point}"
log "Pinning $iso_filename for Ubuntu $VERSION (point $point)"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

log "Fetching SHA256SUMS..."
curl -fsSL "$sums_url" -o "$tmp/SHA256SUMS"
log "Fetching SHA256SUMS.gpg..."
curl -fsSL "$gpg_url" -o "$tmp/SHA256SUMS.gpg"

# Ensure the archive key is in the keyring (best-effort fetch from keyservers)
if ! gpg --list-keys "$fp" >/dev/null 2>&1; then
    log "Importing Ubuntu archive key $fp..."
    gpg --keyserver keyserver.ubuntu.com --recv-keys "$fp" 2>/dev/null || \
        gpg --keyserver hkps://keys.openpgp.org --recv-keys "$fp" 2>/dev/null || {
        log "FATAL: could not fetch GPG key $fp"
        exit 1
    }
fi

log "Verifying SHA256SUMS against GPG signature..."
gpg --verify "$tmp/SHA256SUMS.gpg" "$tmp/SHA256SUMS" 2>&1 | grep -q "Good signature" || {
    log "FATAL: SHA256SUMS GPG verification failed"
    exit 1
}
log "  GPG verification OK"

# Confirm signer fingerprint matches pinned value. gpg formats the
# fingerprint as five groups of 4 hex, two spaces, five more groups
# (e.g. "8439 38DF 228D 22F7 B374  2BC0 D94A A3F0 EFE2 1092"). The
# original `([A-F0-9]{4} ){9}[A-F0-9]{4}` regex assumed single spaces
# and never matched. Allow one-or-more spaces between groups.
signer_fp=$(gpg --verify "$tmp/SHA256SUMS.gpg" "$tmp/SHA256SUMS" 2>&1 | \
    grep -oE "[A-F0-9]{4}( +[A-F0-9]{4}){9}" | tr -d ' ' | head -1)
if [[ "${signer_fp^^}" != "${fp^^}" ]]; then
    log "FATAL: signer fingerprint mismatch"
    log "  expected: $fp"
    log "  got:      $signer_fp"
    exit 1
fi
log "  Signer fingerprint matches pinned value"

# Extract sha256 for the live-server ISO
sha=$(grep "$iso_filename" "$tmp/SHA256SUMS" | awk '{print $1}')
if [[ -z "$sha" ]]; then
    log "FATAL: $iso_filename not found in SHA256SUMS"
    cat "$tmp/SHA256SUMS"
    exit 1
fi
log "  ISO sha256: $sha"

# Write back into pins file
now=$(date -u -Iseconds)
updated=$(jq \
    --arg v "$VERSION" \
    --arg sha "$sha" \
    --arg ts "$now" \
    '.releases[$v].sha256 = $sha | .releases[$v].pinned_at = $ts' \
    "$PINS_FILE")

echo "$updated" | jq . > "$PINS_FILE"
log "Updated $PINS_FILE — commit the diff with a 'chore(iso): pin Ubuntu $point' message."
