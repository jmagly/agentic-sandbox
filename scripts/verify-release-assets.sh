#!/usr/bin/env bash
# Verify release publication surfaces after a production tag run.

set -euo pipefail

TAG=""
GITHUB_REPO="${GITHUB_REPO:-jmagly/agentic-sandbox}"
GHCR_OWNER="${GHCR_OWNER:-jmagly}"
SKIP_GHCR=false
SKIP_DOWNLOAD=false

usage() {
  cat <<'USAGE'
usage: verify-release-assets.sh <vYYYY.M.P> [options]

Options:
  --github-repo <owner/repo>  GitHub mirror repository (default: jmagly/agentic-sandbox)
  --ghcr-owner <owner>        GHCR namespace owner (default: jmagly)
  --skip-ghcr                Do not docker pull/run GHCR images
  --skip-download            Do not download release assets or run installer dry-runs
  -h, --help                 Show this help

Environment:
  GITHUB_REPO                Default GitHub mirror repository
  GHCR_OWNER                 Default GHCR namespace owner
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --github-repo) GITHUB_REPO="${2:-}"; shift 2 ;;
    --ghcr-owner) GHCR_OWNER="${2:-}"; shift 2 ;;
    --skip-ghcr) SKIP_GHCR=true; shift ;;
    --skip-download) SKIP_DOWNLOAD=true; shift ;;
    v[0-9]*.[0-9]*.[0-9]*)
      if [ -n "$TAG" ]; then
        echo "tag specified more than once" >&2
        exit 2
      fi
      TAG="$1"
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

[ -n "$TAG" ] || { usage >&2; exit 2; }

case "$TAG" in
  v[0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "tag must look like v2026.6.2" >&2; exit 2 ;;
esac

VERSION="${TAG#v}"
TMPDIR_RELEASE="$(mktemp -d -t agentic-release-verify.XXXXXX)"
trap 'rm -rf "$TMPDIR_RELEASE"' EXIT

info() { printf '[release-verify] %s\n' "$*"; }
die() { printf '[release-verify] %s\n' "$*" >&2; exit 1; }

command -v gh >/dev/null || die "gh CLI is required"
command -v python3 >/dev/null || die "python3 is required"

info "reading GitHub release ${GITHUB_REPO}@${TAG}"
ASSETS_JSON="$(gh release view "$TAG" --repo "$GITHUB_REPO" --json assets)"

require_asset() {
  local name="$1"
  ASSETS_JSON="$ASSETS_JSON" python3 - "$name" <<'PY'
import json
import os
import sys

target = sys.argv[1]
data = json.loads(os.environ["ASSETS_JSON"])
assets = {asset.get("name") for asset in data.get("assets", [])}
if target not in assets:
    sys.stderr.write(f"missing release asset: {target}\n")
    sys.exit(1)
PY
}

required_assets=(
  "agentic-sandbox_${VERSION}-1_amd64.deb"
  "agentic-sandbox_${VERSION}-1_amd64.deb.sha256"
  "agentic-sandbox-${VERSION}-1.x86_64.rpm"
  "agentic-sandbox-${VERSION}-1.x86_64.rpm.sha256"
  "agentic-sandbox-install.sh"
  "agentic-sandbox-install.sh.sha256"
  "SHA256SUMS-linux-packages"
  "SHA256SUMS"
  "agentic-sandbox-${TAG}-x86_64-linux-gnu.tar.gz"
  "agentic-sandbox-${TAG}-x86_64-linux-gnu.tar.gz.sha256"
  "agentic-sandbox-${TAG}-x86_64-linux-musl.tar.gz"
  "agentic-sandbox-${TAG}-x86_64-linux-musl.tar.gz.sha256"
  "agentic-sandbox-${TAG}-aarch64-darwin.tar.gz"
  "agentic-sandbox-${TAG}-aarch64-darwin.tar.gz.sha256"
)

for asset in "${required_assets[@]}"; do
  require_asset "$asset"
done
info "GitHub release asset names are complete"

if [ "$SKIP_DOWNLOAD" != "true" ]; then
  command -v curl >/dev/null || die "curl is required"
  command -v sha256sum >/dev/null || die "sha256sum is required"

  base="https://github.com/${GITHUB_REPO}/releases/download/${TAG}"
  info "downloading package assets for checksum verification"
  for asset in \
    "agentic-sandbox_${VERSION}-1_amd64.deb" \
    "agentic-sandbox-${VERSION}-1.x86_64.rpm" \
    "agentic-sandbox-install.sh" \
    "SHA256SUMS-linux-packages"; do
    curl -fsSL -o "${TMPDIR_RELEASE}/${asset}" "${base}/${asset}"
  done

  ( cd "$TMPDIR_RELEASE" && sha256sum -c SHA256SUMS-linux-packages )

  info "verifying installer dry-runs from GitHub release URL"
  AGENTIC_RELEASE_BASE="https://github.com/${GITHUB_REPO}" \
    AGENTIC_RELEASE_API="https://api.github.com/repos/${GITHUB_REPO}" \
    bash scripts/install.sh --version "$TAG" --package deb --dry-run
  AGENTIC_RELEASE_BASE="https://github.com/${GITHUB_REPO}" \
    AGENTIC_RELEASE_API="https://api.github.com/repos/${GITHUB_REPO}" \
    bash scripts/install.sh --version "$TAG" --package rpm --dry-run
fi

if [ "$SKIP_GHCR" != "true" ]; then
  command -v docker >/dev/null || die "docker is required for GHCR verification"
  info "verifying GHCR release images"
  for image in \
    agentic-sandbox-mgmt \
    agentic-sandbox-agent-client \
    agentic-sandbox-agent \
    agentic-sandbox-claude \
    agentic-sandbox-codex \
    agentic-sandbox-opencode \
    agentic-sandbox-automation-control; do
    ref="ghcr.io/${GHCR_OWNER}/${image}:${TAG}"
    docker pull "$ref"
  done

  docker run --rm --entrypoint /bin/sh "ghcr.io/${GHCR_OWNER}/agentic-sandbox-mgmt:${TAG}" \
    -lc 'command -v agentic-mgmt >/dev/null && test -x "$(command -v agentic-mgmt)"'
  docker run --rm --entrypoint /usr/local/bin/agent-client \
    "ghcr.io/${GHCR_OWNER}/agentic-sandbox-agent-client:${TAG}" --help >/dev/null
fi

info "release verification passed for ${TAG}"
