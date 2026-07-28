#!/usr/bin/env bash
# Validate and create a signed Agentic Sandbox release tag. This script never
# pushes; the operator reviews the verified tag before publishing either remote.

set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: tools/release/cut-tag.sh <YYYY.M.PATCH> [-m <message>]
EOF
}

[[ $# -ge 1 ]] || { usage; exit 2; }
version="$1"
shift
message="v${version}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    -m|--message)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      message="$2"
      shift 2
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

[[ "$version" =~ ^[0-9]{4}\.([1-9]|1[0-2])\.([0-9]|[1-9][0-9]+)$ ]] \
  || { echo "invalid CalVer: $version" >&2; exit 1; }

tag="v${version}"
[[ "$(git branch --show-current)" == "main" ]] \
  || { echo "release tags must be cut from main" >&2; exit 1; }
[[ -z "$(git status --porcelain)" ]] \
  || { echo "working tree must be clean before tagging" >&2; exit 1; }
git rev-parse --verify HEAD >/dev/null
if git rev-parse --verify "refs/tags/${tag}" >/dev/null 2>&1; then
  echo "tag already exists locally: ${tag}" >&2
  exit 1
fi

for manifest in management/Cargo.toml agent-rs/Cargo.toml cli/Cargo.toml; do
  actual="$(awk -F '"' '$1 == "version = " {print $2; exit}' "$manifest")"
  [[ "$actual" == "$version" ]] \
    || { echo "$manifest version $actual does not match $version" >&2; exit 1; }
done

for lockfile in management/Cargo.lock agent-rs/Cargo.lock cli/Cargo.lock; do
  grep -qF "version = \"${version}\"" "$lockfile" \
    || { echo "$lockfile does not contain version $version" >&2; exit 1; }
done

grep -qF "## [${version}]" CHANGELOG.md \
  || { echo "CHANGELOG.md is missing ${version}" >&2; exit 1; }
[[ -f "docs/releases/v${version}.md" ]] \
  || { echo "docs/releases/v${version}.md is missing" >&2; exit 1; }

signing_key="${AGENTIC_RELEASE_TAG_KEY:-$(git config --get user.signingkey || true)}"
[[ -n "$signing_key" ]] \
  || { echo "no tag-signing key is configured" >&2; exit 1; }
gpg --list-secret-keys "$signing_key" >/dev/null 2>&1 \
  || { echo "configured tag-signing key is unavailable" >&2; exit 1; }

git tag -s -u "$signing_key" "$tag" -m "$message"
git tag -v "$tag"

printf 'signed tag verified: %s -> %s\n' "$tag" "$(git rev-parse HEAD)"
printf 'next: git push origin %s && git push github %s\n' "$tag" "$tag"
