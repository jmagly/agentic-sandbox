#!/usr/bin/env bash
# Build and publish the Apple-compatible agent image chain as OCI indexes.
set -euo pipefail

platforms="${AGENT_IMAGE_PLATFORMS:-linux/amd64,linux/arm64}"
registry="${AGENT_IMAGE_REGISTRY:-}"
revision="${AGENT_IMAGE_REVISION:-$(git rev-parse HEAD)}"
release_tag="${AGENT_IMAGE_RELEASE_TAG:-}"
evidence="${AGENT_IMAGE_EVIDENCE:-multiarch-agent-images.jsonl}"
inspect_attempts="${AGENT_IMAGE_INSPECT_ATTEMPTS:-12}"
inspect_delay_seconds="${AGENT_IMAGE_INSPECT_DELAY_SECONDS:-5}"
cargo_build_jobs="${AGENT_IMAGE_CARGO_BUILD_JOBS:-8}"
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
if ! [[ "$inspect_attempts" =~ ^[1-9][0-9]*$ ]]; then
  echo "AGENT_IMAGE_INSPECT_ATTEMPTS must be a positive integer" >&2
  exit 2
fi
if ! [[ "$inspect_delay_seconds" =~ ^[0-9]+$ ]]; then
  echo "AGENT_IMAGE_INSPECT_DELAY_SECONDS must be a non-negative integer" >&2
  exit 2
fi
if ! [[ "$cargo_build_jobs" =~ ^[1-9][0-9]*$ ]]; then
  echo "AGENT_IMAGE_CARGO_BUILD_JOBS must be a positive integer" >&2
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
    # Preserve the pre-multiarch public contract: the generic agent revision
    # and release tags resolve to the developer image, while the explicit
    # agent:base-* and agent:dev-* tags retain the build-chain identities.
    # Release mirroring and cosign signing consume these generic aliases.
    if [[ "$name" == "agent" && "$channel" == "dev" ]]; then
      TAGS+=(--tag "${registry}/${name}:${revision}")
      if [[ -n "$release_tag" ]]; then TAGS+=(--tag "${registry}/${name}:${release_tag}"); fi
    fi
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

inspect_with_retry() {
  local mode="$1" ref="$2" attempt output
  local -a command=(docker buildx imagetools inspect)
  if [[ "$mode" == "raw" ]]; then
    command+=(--raw)
  fi

  for ((attempt = 1; attempt <= inspect_attempts; attempt++)); do
    if output="$("${command[@]}" "$ref" 2>&1)"; then
      printf '%s\n' "$output"
      return 0
    fi

    echo "manifest inspection unavailable for $ref (attempt $attempt/$inspect_attempts)" >&2
    if ((attempt == inspect_attempts)); then
      printf '%s\n' "$output" >&2
      return 1
    fi
    sleep "$inspect_delay_seconds"
  done
}

base_ref="${registry}/agent:base-${revision}"
dev_ref="${registry}/agent:dev-${revision}"
codex_ref="${registry}/codex:${revision}"

build_image agent base images/container/Dockerfile.base --build-arg "CARGO_BUILD_JOBS=${cargo_build_jobs}"
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
    raw="$(inspect_with_retry raw "$ref")"
    digest="$(inspect_with_retry summary "$ref" | awk '/^Digest:/ {print $2; exit}')"
    if [[ -z "$digest" ]]; then
      echo "manifest digest missing for $ref" >&2
      exit 1
    fi
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
