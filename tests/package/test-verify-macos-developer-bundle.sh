#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

tag="v2026.7.14"
source_commit="b60c23719580b8d377bc263ad91a0a11d6472198"
base="agentic-sandbox-${tag}-aarch64-darwin"
bundle="$work/bundle"
output="$work/output"
mkdir -p "$bundle/package-source"

printf 'source archive\n' > "$bundle/source.tar.gz"
printf 'unsigned developer package\n' > "$bundle/${base}-preview.pkg"
printf 'path\tmode\tsha256\n' > "$bundle/${base}.payload-manifest.tsv"

payloads=(
  "agentic-mgmt"
  "agentic-host-runtime-daemon"
  "sandboxctl"
  "agent-client"
)
for payload in "${payloads[@]}"; do
  printf 'payload:%s\n' "$payload" > "$bundle/package-source/$payload"
done

digest() {
  shasum -a 256 "$1" | awk '{print $1}'
}

package_sha256="$(digest "$bundle/${base}-preview.pkg")"
manifest_sha256="$(digest "$bundle/${base}.payload-manifest.tsv")"
source_archive_sha256="$(digest "$bundle/source.tar.gz")"
payloads_json="$(
  for payload in "${payloads[@]}"; do
    jq -nc \
      --arg name "$payload" \
      --arg sha256 "$(digest "$bundle/package-source/$payload")" \
      '{name:$name,sha256:$sha256}'
  done | jq -s .
)"

jq -n \
  --arg source_commit "$source_commit" \
  --arg release_tag "$tag" \
  --arg source_archive_sha256 "$source_archive_sha256" \
  --arg preview_package_sha256 "$package_sha256" \
  --arg preview_manifest_sha256 "$manifest_sha256" \
  --argjson payloads "$payloads_json" \
  '{
    schema_version:"agentic.macos-release-approval-bundle.v1",
    source_commit:$source_commit,
    release_tag:$release_tag,
    source_archive_sha256:$source_archive_sha256,
    preview_package_sha256:$preview_package_sha256,
    preview_manifest_sha256:$preview_manifest_sha256,
    payloads:$payloads,
    immutable:true,
    credential_contents_retained:false
  }' > "$bundle/prepare-evidence.json"

"$repo_root/scripts/verify-macos-developer-bundle.sh" \
  --bundle-dir "$bundle" \
  --expected-package-sha256 "$package_sha256" \
  --expected-manifest-sha256 "$manifest_sha256" \
  --source-commit "$source_commit" \
  --release-tag "$tag" \
  --output-dir "$output"

developer_base="${base}-developer-unsigned"
test -f "$output/${developer_base}.pkg"
test -f "$output/${developer_base}.pkg.sha256"
test -f "$output/${developer_base}.payload-manifest.tsv"
test -f "$output/${developer_base}.evidence.json"
test -f "$output/SHA256SUMS-macos-developer"
(
  cd "$output"
  shasum -a 256 -c SHA256SUMS-macos-developer >/dev/null
  shasum -a 256 -c "${developer_base}.pkg.sha256" >/dev/null
)
jq -e \
  --arg source_commit "$source_commit" \
  --arg release_tag "$tag" \
  --arg package_sha256 "$package_sha256" '
    .schema_version == "agentic.macos-developer-release.v1" and
    .source_commit == $source_commit and
    .release_tag == $release_tag and
    .package.sha256 == $package_sha256 and
    .package.developer_unsigned == true and
    .package.signed == false and
    .package.notarized == false and
    .package.stapled == false and
    .immutable_source_bundle == true and
    .credential_contents_retained == false
  ' "$output/${developer_base}.evidence.json" >/dev/null

tampered="$work/tampered"
cp -R "$bundle" "$tampered"
printf 'tampered\n' >> "$tampered/${base}-preview.pkg"
if "$repo_root/scripts/verify-macos-developer-bundle.sh" \
  --bundle-dir "$tampered" \
  --expected-package-sha256 "$package_sha256" \
  --expected-manifest-sha256 "$manifest_sha256" \
  --source-commit "$source_commit" \
  --release-tag "$tag" \
  --output-dir "$work/tampered-output" >/dev/null 2>&1; then
  echo "tampered developer package was accepted" >&2
  exit 1
fi

tampered_evidence="$work/tampered-evidence"
cp -R "$bundle" "$tampered_evidence"
jq '.source_commit = "0000000000000000000000000000000000000000"' \
  "$tampered_evidence/prepare-evidence.json" \
  > "$tampered_evidence/prepare-evidence.json.tmp"
mv "$tampered_evidence/prepare-evidence.json.tmp" \
  "$tampered_evidence/prepare-evidence.json"
if "$repo_root/scripts/verify-macos-developer-bundle.sh" \
  --bundle-dir "$tampered_evidence" \
  --expected-package-sha256 "$package_sha256" \
  --expected-manifest-sha256 "$manifest_sha256" \
  --source-commit "$source_commit" \
  --release-tag "$tag" \
  --output-dir "$work/tampered-evidence-output" >/dev/null 2>&1; then
  echo "tampered developer evidence was accepted" >&2
  exit 1
fi

open_bundle="$work/open-bundle"
cp -R "$bundle" "$open_bundle"
printf 'unexpected\n' > "$open_bundle/unexpected.txt"
if "$repo_root/scripts/verify-macos-developer-bundle.sh" \
  --bundle-dir "$open_bundle" \
  --expected-package-sha256 "$package_sha256" \
  --expected-manifest-sha256 "$manifest_sha256" \
  --source-commit "$source_commit" \
  --release-tag "$tag" \
  --output-dir "$work/open-bundle-output" >/dev/null 2>&1; then
  echo "developer bundle with an unexpected file was accepted" >&2
  exit 1
fi

echo "macOS developer bundle verification: ok"
