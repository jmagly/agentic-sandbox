#!/usr/bin/env bash
# Verify and stage immutable, already-signed Apple release bytes for tag CI.

set -euo pipefail

HANDOFF_DIR=""
SOURCE_COMMIT=""
RELEASE_TAG=""
EXPECTED_EVIDENCE_SHA256=""
OUTPUT_DIR=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-macos-release-handoff.sh \
  --handoff-dir <dir> --expected-evidence-sha256 <sha256> \
  --source-commit <sha> --release-tag <vCalVer> \
  --output-dir <empty-dir>

The exact handoff directory must match an independently approved evidence digest.
This verifier never signs, notarizes, mutates, or replaces Apple artifacts.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --handoff-dir) HANDOFF_DIR="${2:-}"; shift 2 ;;
    --expected-evidence-sha256) EXPECTED_EVIDENCE_SHA256="${2:-}"; shift 2 ;;
    --source-commit) SOURCE_COMMIT="${2:-}"; shift 2 ;;
    --release-tag) RELEASE_TAG="${2:-}"; shift 2 ;;
    --output-dir) OUTPUT_DIR="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -d "$HANDOFF_DIR" ]] || { echo "macOS release handoff is unavailable" >&2; exit 1; }
[[ "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid source commit" >&2; exit 2; }
[[ "$RELEASE_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "invalid release tag" >&2; exit 2; }
[[ "$EXPECTED_EVIDENCE_SHA256" =~ ^[0-9a-f]{64}$ ]] \
  || { echo "invalid independently approved evidence digest" >&2; exit 2; }
[[ -n "$OUTPUT_DIR" ]] || { echo "missing --output-dir" >&2; exit 2; }
[[ ! -e "$OUTPUT_DIR" ]] || {
  [[ -d "$OUTPUT_DIR" && -z "$(find "$OUTPUT_DIR" -mindepth 1 -print -quit)" ]] \
    || { echo "output directory must be absent or empty" >&2; exit 1; }
}

handoff="$HANDOFF_DIR"
handoff_digest="${handoff##*/}"
[[ "$handoff_digest" == "$EXPECTED_EVIDENCE_SHA256" ]] \
  || { echo "macOS release handoff differs from approved evidence digest" >&2; exit 1; }

version="${RELEASE_TAG#v}"
base="agentic-sandbox-v${version}-aarch64-darwin"
pkg="${base}.pkg"
dmg="${base}.dmg"
manifest="${base}.payload-manifest.tsv"
evidence="${base}.release-evidence.json"
expected_files=(
  "$pkg"
  "$dmg"
  "$pkg.sha256"
  "$dmg.sha256"
  "SHA256SUMS-macos"
  "$manifest"
  "$evidence"
  "handoff.json"
)

if find "$handoff" -mindepth 1 -maxdepth 1 ! -type f -print -quit | grep -q .; then
  echo "macOS release handoff contains a non-regular entry" >&2
  exit 1
fi
actual_file_count=0
while IFS= read -r -d '' path; do
  name="${path##*/}"
  case "$name" in
    "$pkg"|"$dmg"|"$pkg.sha256"|"$dmg.sha256"|"SHA256SUMS-macos"|"$manifest"|"$evidence"|"handoff.json")
      ;;
    *)
      echo "macOS release handoff file set is not closed" >&2
      exit 1
      ;;
  esac
  actual_file_count=$((actual_file_count + 1))
done < <(find "$handoff" -mindepth 1 -maxdepth 1 -type f -print0)
[[ "$actual_file_count" -eq "${#expected_files[@]}" ]] \
  || { echo "macOS release handoff file set is not closed" >&2; exit 1; }

jq -e \
  --arg release_tag "$RELEASE_TAG" \
  --arg source_commit "$SOURCE_COMMIT" \
  --arg evidence_sha256 "$handoff_digest" '
    (keys | sort) == [
      "credential_contents_retained",
      "immutable",
      "release_evidence_sha256",
      "release_tag",
      "schema_version",
      "source_commit"
    ] and
    .schema_version == "agentic.macos-release-handoff.v1" and
    .release_tag == $release_tag and
    .source_commit == $source_commit and
    .release_evidence_sha256 == $evidence_sha256 and
    .immutable == true and
    .credential_contents_retained == false
  ' "$handoff/handoff.json" >/dev/null \
  || { echo "macOS release handoff metadata is invalid" >&2; exit 1; }

actual_evidence_sha256="$(shasum -a 256 "$handoff/$evidence" | awk '{print $1}')"
[[ "$actual_evidence_sha256" == "$handoff_digest" ]] \
  || { echo "macOS release evidence digest does not match its immutable path" >&2; exit 1; }

root="$(cd "$(dirname "$0")/.." && pwd)"
"$root/scripts/validate-macos-release-evidence.py" \
  "$handoff/$evidence" \
  --expect-source-commit "$SOURCE_COMMIT" \
  --expect-tag "$RELEASE_TAG" >/dev/null

(
  cd "$handoff"
  shasum -a 256 -c "SHA256SUMS-macos" >/dev/null
  shasum -a 256 -c "$pkg.sha256" >/dev/null
  shasum -a 256 -c "$dmg.sha256" >/dev/null
)

for tuple in "pkg:$pkg" "dmg:$dmg"; do
  kind="${tuple%%:*}"
  name="${tuple#*:}"
  expected_sha256="$(
    jq -er --arg kind "$kind" '.artifacts[] | select(.kind == $kind) | .sha256' \
      "$handoff/$evidence"
  )"
  actual_sha256="$(shasum -a 256 "$handoff/$name" | awk '{print $1}')"
  [[ "$actual_sha256" == "$expected_sha256" ]] \
    || { echo "macOS artifact digest differs from verified evidence" >&2; exit 1; }
done

expected_manifest_sha256="$(
  jq -er '.package.signed_payload_manifest.sha256' "$handoff/$evidence"
)"
actual_manifest_sha256="$(shasum -a 256 "$handoff/$manifest" | awk '{print $1}')"
[[ "$actual_manifest_sha256" == "$expected_manifest_sha256" ]] \
  || { echo "signed payload manifest differs from verified evidence" >&2; exit 1; }

mkdir -p "$OUTPUT_DIR"
for file in "${expected_files[@]}"; do
  install -m 0644 "$handoff/$file" "$OUTPUT_DIR/"
done
echo "macOS release handoff: verified"
