#!/usr/bin/env bash
# Build and publish the Apple-compatible agent image chain as OCI indexes.
set -euo pipefail

platforms="${AGENT_IMAGE_PLATFORMS:-linux/amd64,linux/arm64}"
registry="${AGENT_IMAGE_REGISTRY:-}"
revision="${AGENT_IMAGE_REVISION:-$(git rev-parse HEAD)}"
release_tag="${AGENT_IMAGE_RELEASE_TAG:-}"
evidence="${AGENT_IMAGE_EVIDENCE:-multiarch-agent-images.jsonl}"
dry_run=false

usage() {
  echo "usage: $0 --registry REGISTRY/PREFIX [--revision TAG] [--release-tag TAG] [--platforms LIST] [--evidence FILE] [--dry-run]" >&2
}

while (($#)); do
  case "$1" in
    --registry) registry="$2"; shift 2 ;;
    --revision) revision="$2"; shift 2 ;;
    --release-tag) release_tag="$2"; shift 2 ;;
    --platforms) platforms="$2"; shift 2 ;;
    --evidence) evidence="$2"; shift 2 ;;
    --dry-run) dry_run=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done

if [[ -z "$registry" ]]; then
  echo "--registry is required" >&2
  exit 2
fi
if [[ "$platforms" != *linux/amd64* || "$platforms" != *linux/arm64* ]]; then
  echo "platforms must include linux/amd64 and linux/arm64" >&2
  exit 2
fi

run() {
  printf '+ '
  printf '%s ' "$@"
  printf '\n'
  if [[ "$dry_run" == false ]]; then "$@"; fi
}

tags_for() {
  local name="$1" channel="$2"
  if [[ "$channel" == "latest" ]]; then
    TAGS=(--tag "${registry}/${name}:latest" --tag "${registry}/${name}:${revision}")
    if [[ -n "$release_tag" ]]; then TAGS+=(--tag "${registry}/${name}:${release_tag}"); fi
  else
    TAGS=(--tag "${registry}/${name}:${channel}" --tag "${registry}/${name}:${channel}-${revision}")
    if [[ -n "$release_tag" ]]; then TAGS+=(--tag "${registry}/${name}:${channel}-${release_tag}"); fi
  fi
}

build_image() {
  local name="$1" channel="$2" dockerfile="$3"; shift 3
  tags_for "$name" "$channel"
  run docker buildx build \
    --platform "$platforms" \
    --file "$dockerfile" \
    --provenance=true \
    --sbom=true \
    "${TAGS[@]}" \
    "$@" \
    --push .
}

base_ref="${registry}/agent:base-${revision}"
dev_ref="${registry}/agent:dev-${revision}"
codex_ref="${registry}/codex:${revision}"

build_image agent base images/container/Dockerfile.base
build_image agent dev images/container/Dockerfile.dev --build-arg "AGENT_BASE_IMAGE=${base_ref}"
build_image claude latest images/container/Dockerfile.claude --build-arg "AGENT_DEV_IMAGE=${dev_ref}"
build_image codex latest images/container/Dockerfile.codex --build-arg "AGENT_DEV_IMAGE=${dev_ref}"
build_image opencode latest images/container/Dockerfile.opencode --build-arg "AGENT_DEV_IMAGE=${dev_ref}"
build_image automation-control latest images/container/Dockerfile.automation-control --build-arg "CODEX_IMAGE=${codex_ref}"

if [[ "$dry_run" == false ]]; then
  : > "$evidence"
  for ref in \
    "$base_ref" "$dev_ref" \
    "${registry}/claude:${revision}" \
    "${registry}/codex:${revision}" \
    "${registry}/opencode:${revision}" \
    "${registry}/automation-control:${revision}"; do
    raw="$(docker buildx imagetools inspect --raw "$ref")"
    digest="$(docker buildx imagetools inspect "$ref" | awk '/^Digest:/ {print $2; exit}')"
    jq -ce --arg ref "$ref" --arg digest "$digest" '
      {ref: $ref, manifest_digest: $digest,
       platforms: [.manifests[] | {platform: (.platform.os + "/" + .platform.architecture), digest: .digest}]}
      | select((.platforms | map(.platform) | index("linux/amd64")) != null)
      | select((.platforms | map(.platform) | index("linux/arm64")) != null)
    ' <<<"$raw" >> "$evidence" || {
      echo "missing required platform manifest for $ref" >&2
      exit 1
    }
  done
fi
