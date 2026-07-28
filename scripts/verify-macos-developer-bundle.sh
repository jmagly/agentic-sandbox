#!/usr/bin/env bash
# Verify and stage an immutable unsigned macOS developer package for release.

set -euo pipefail

BUNDLE_DIR=""
SOURCE_COMMIT=""
RELEASE_TAG=""
EXPECTED_PACKAGE_SHA256=""
EXPECTED_MANIFEST_SHA256=""
OUTPUT_DIR=""

usage() {
  cat <<'EOF'
Usage: scripts/verify-macos-developer-bundle.sh \
  --bundle-dir <dir> \
  --expected-package-sha256 <sha256> \
  --expected-manifest-sha256 <sha256> \
  --source-commit <sha> \
  --release-tag <vCalVer> \
  --output-dir <empty-dir>

The prepared bundle must be the immutable output of the credential-free macOS
prepare ceremony. This verifier does not sign, notarize, or alter its payload.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bundle-dir) BUNDLE_DIR="${2:-}"; shift 2 ;;
    --expected-package-sha256) EXPECTED_PACKAGE_SHA256="${2:-}"; shift 2 ;;
    --expected-manifest-sha256) EXPECTED_MANIFEST_SHA256="${2:-}"; shift 2 ;;
    --source-commit) SOURCE_COMMIT="${2:-}"; shift 2 ;;
    --release-tag) RELEASE_TAG="${2:-}"; shift 2 ;;
    --output-dir) OUTPUT_DIR="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -d "$BUNDLE_DIR" ]] || { echo "macOS developer bundle is unavailable" >&2; exit 1; }
[[ "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid source commit" >&2; exit 2; }
[[ "$RELEASE_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || { echo "invalid release tag" >&2; exit 2; }
[[ "$EXPECTED_PACKAGE_SHA256" =~ ^[0-9a-f]{64}$ ]] \
  || { echo "invalid approved developer package digest" >&2; exit 2; }
[[ "$EXPECTED_MANIFEST_SHA256" =~ ^[0-9a-f]{64}$ ]] \
  || { echo "invalid approved developer manifest digest" >&2; exit 2; }
[[ -n "$OUTPUT_DIR" ]] || { echo "missing --output-dir" >&2; exit 2; }
[[ ! -e "$OUTPUT_DIR" ]] || {
  [[ -d "$OUTPUT_DIR" && -z "$(find "$OUTPUT_DIR" -mindepth 1 -print -quit)" ]] \
    || { echo "output directory must be absent or empty" >&2; exit 1; }
}

version="${RELEASE_TAG#v}"
preview_base="agentic-sandbox-v${version}-aarch64-darwin"
preview_pkg="${preview_base}-preview.pkg"
preview_manifest="${preview_base}.payload-manifest.tsv"
prepare_evidence="prepare-evidence.json"
package_source="package-source"

expected_top_level=(
  "source.tar.gz"
  "$preview_pkg"
  "$preview_manifest"
  "$prepare_evidence"
  "$package_source"
)

actual_entry_count=0
while IFS= read -r -d '' path; do
  name="${path##*/}"
  case "$name" in
    "source.tar.gz"|"$preview_pkg"|"$preview_manifest"|"$prepare_evidence"|"$package_source")
      ;;
    *)
      echo "macOS developer bundle file set is not closed" >&2
      exit 1
      ;;
  esac
  actual_entry_count=$((actual_entry_count + 1))
done < <(find "$BUNDLE_DIR" -mindepth 1 -maxdepth 1 -print0)
[[ "$actual_entry_count" -eq "${#expected_top_level[@]}" ]] \
  || { echo "macOS developer bundle file set is not closed" >&2; exit 1; }

for file in "source.tar.gz" "$preview_pkg" "$preview_manifest" "$prepare_evidence"; do
  [[ -f "$BUNDLE_DIR/$file" && ! -L "$BUNDLE_DIR/$file" ]] \
    || { echo "macOS developer bundle contains an invalid file" >&2; exit 1; }
done
[[ -d "$BUNDLE_DIR/$package_source" && ! -L "$BUNDLE_DIR/$package_source" ]] \
  || { echo "macOS developer payload directory is invalid" >&2; exit 1; }

payload_names=(
  "agentic-mgmt"
  "agentic-host-runtime-daemon"
  "sandboxctl"
  "agent-client"
)
actual_payload_count=0
while IFS= read -r -d '' path; do
  name="${path##*/}"
  case "$name" in
    "agentic-mgmt"|"agentic-host-runtime-daemon"|"sandboxctl"|"agent-client")
      ;;
    *)
      echo "macOS developer payload file set is not closed" >&2
      exit 1
      ;;
  esac
  [[ -f "$path" && ! -L "$path" ]] \
    || { echo "macOS developer payload contains an invalid file" >&2; exit 1; }
  actual_payload_count=$((actual_payload_count + 1))
done < <(find "$BUNDLE_DIR/$package_source" -mindepth 1 -maxdepth 1 -print0)
[[ "$actual_payload_count" -eq "${#payload_names[@]}" ]] \
  || { echo "macOS developer payload file set is not closed" >&2; exit 1; }

jq -e \
  --arg source_commit "$SOURCE_COMMIT" \
  --arg release_tag "$RELEASE_TAG" \
  --arg preview_package_sha256 "$EXPECTED_PACKAGE_SHA256" \
  --arg preview_manifest_sha256 "$EXPECTED_MANIFEST_SHA256" '
    (keys | sort) == [
      "credential_contents_retained",
      "immutable",
      "payloads",
      "preview_manifest_sha256",
      "preview_package_sha256",
      "release_tag",
      "schema_version",
      "source_archive_sha256",
      "source_commit"
    ] and
    .schema_version == "agentic.macos-release-approval-bundle.v1" and
    .source_commit == $source_commit and
    .release_tag == $release_tag and
    .preview_package_sha256 == $preview_package_sha256 and
    .preview_manifest_sha256 == $preview_manifest_sha256 and
    .immutable == true and
    .credential_contents_retained == false and
    (.payloads | map(.name)) == [
      "agentic-mgmt",
      "agentic-host-runtime-daemon",
      "sandboxctl",
      "agent-client"
    ] and
    (.payloads | all(
      (keys | sort) == ["name","sha256"] and
      (.sha256 | test("^[0-9a-f]{64}$"))
    ))
  ' "$BUNDLE_DIR/$prepare_evidence" >/dev/null \
  || { echo "macOS developer approval metadata is invalid" >&2; exit 1; }

digest() {
  shasum -a 256 "$1" | awk '{print $1}'
}

[[ "$(digest "$BUNDLE_DIR/$preview_pkg")" == "$EXPECTED_PACKAGE_SHA256" ]] \
  || { echo "macOS developer package digest mismatch" >&2; exit 1; }
[[ "$(digest "$BUNDLE_DIR/$preview_manifest")" == "$EXPECTED_MANIFEST_SHA256" ]] \
  || { echo "macOS developer manifest digest mismatch" >&2; exit 1; }
[[ "$(digest "$BUNDLE_DIR/source.tar.gz")" == \
  "$(jq -er '.source_archive_sha256' "$BUNDLE_DIR/$prepare_evidence")" ]] \
  || { echo "macOS developer source archive digest mismatch" >&2; exit 1; }

for payload in "${payload_names[@]}"; do
  expected_payload_sha256="$(
    jq -er --arg name "$payload" \
      '.payloads[] | select(.name == $name) | .sha256' \
      "$BUNDLE_DIR/$prepare_evidence"
  )"
  [[ "$(digest "$BUNDLE_DIR/$package_source/$payload")" == "$expected_payload_sha256" ]] \
    || { echo "macOS developer payload digest mismatch: $payload" >&2; exit 1; }
done

developer_base="${preview_base}-developer-unsigned"
developer_pkg="${developer_base}.pkg"
developer_manifest="${developer_base}.payload-manifest.tsv"
developer_evidence="${developer_base}.evidence.json"

mkdir -p "$OUTPUT_DIR"
install -m 0644 "$BUNDLE_DIR/$preview_pkg" "$OUTPUT_DIR/$developer_pkg"
install -m 0644 "$BUNDLE_DIR/$preview_manifest" "$OUTPUT_DIR/$developer_manifest"

prepare_evidence_sha256="$(digest "$BUNDLE_DIR/$prepare_evidence")"
source_archive_sha256="$(digest "$BUNDLE_DIR/source.tar.gz")"
jq -n \
  --arg schema_version "agentic.macos-developer-release.v1" \
  --arg source_commit "$SOURCE_COMMIT" \
  --arg release_tag "$RELEASE_TAG" \
  --arg package_name "$developer_pkg" \
  --arg package_sha256 "$EXPECTED_PACKAGE_SHA256" \
  --arg manifest_name "$developer_manifest" \
  --arg manifest_sha256 "$EXPECTED_MANIFEST_SHA256" \
  --arg source_archive_sha256 "$source_archive_sha256" \
  --arg prepare_evidence_sha256 "$prepare_evidence_sha256" \
  --argjson payloads "$(jq -c '.payloads' "$BUNDLE_DIR/$prepare_evidence")" \
  '{
    schema_version:$schema_version,
    source_commit:$source_commit,
    release_tag:$release_tag,
    package:{
      name:$package_name,
      sha256:$package_sha256,
      developer_unsigned:true,
      signed:false,
      notarized:false,
      stapled:false
    },
    payload_manifest:{
      name:$manifest_name,
      sha256:$manifest_sha256
    },
    source_archive_sha256:$source_archive_sha256,
    prepare_evidence_sha256:$prepare_evidence_sha256,
    payloads:$payloads,
    immutable_source_bundle:true,
    credential_contents_retained:false
  }' > "$OUTPUT_DIR/$developer_evidence"

(
  cd "$OUTPUT_DIR"
  shasum -a 256 "$developer_pkg" > "$developer_pkg.sha256"
  shasum -a 256 \
    "$developer_pkg" \
    "$developer_manifest" \
    "$developer_evidence" > SHA256SUMS-macos-developer
)

echo "macOS unsigned developer bundle: verified"
