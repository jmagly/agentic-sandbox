#!/usr/bin/env bash
# Build all agent container images. Builds the base first (every
# platform image FROMs it), then iterates over the platform variants.
#
# Usage:
#   images/container/build.sh                # build base + all platforms
#   images/container/build.sh base           # base only
#   images/container/build.sh claude codex   # specific platforms
#   REGISTRY=ghcr.io/myorg images/container/build.sh --push
#
# Env:
#   REGISTRY   prefix for tagging (e.g. ghcr.io/myorg). Empty = local only.
#   TAG        tag suffix (default: latest)
#
# Run from repo root — Docker build context is `.` so the agent-rs
# release binary at agent-rs/target/release/agent-client is reachable.
#
# Issue: #175

set -euo pipefail

cd "$(dirname "$0")/../.."

PLATFORMS=(claude codex opencode)
# Shared dev toolchain layer between :base and the platform images.
# Built after :base, before any platform image (each platform FROMs :dev).
# Tracked in #182.
DEV_LAYER=dev
TAG="${TAG:-latest}"
REGISTRY="${REGISTRY:-}"
PUSH=0

# --push at any position triggers `docker push` after each build.
ARGS=()
for a in "$@"; do
    case "$a" in
        --push) PUSH=1 ;;
        *) ARGS+=("$a") ;;
    esac
done

# Default to base + dev + all platforms if no specific names passed.
if [[ ${#ARGS[@]} -eq 0 ]]; then
    TARGETS=(base "${DEV_LAYER}" "${PLATFORMS[@]}")
else
    TARGETS=("${ARGS[@]}")
fi

# Verify the agent-client binary is built — base FROM expects it at
# agent-rs/target/release/agent-client. If it's missing, fail loudly
# rather than letting docker copy a stale binary or error out late.
if [[ -n "${TARGETS[*]}" ]]; then
    if [[ ! -x agent-rs/target/release/agent-client ]]; then
        echo "build.sh: agent-rs/target/release/agent-client not found" >&2
        echo "  run: cargo build --release --manifest-path agent-rs/Cargo.toml" >&2
        exit 1
    fi
fi

tag_of() {
    local name="$1"
    if [[ -n "${REGISTRY}" ]]; then
        case "${name}" in
            base) echo "${REGISTRY}/agent:base" ;;
            dev)  echo "${REGISTRY}/agent:dev" ;;
            *)    echo "${REGISTRY}/${name}:${TAG}" ;;
        esac
    else
        case "${name}" in
            base) echo "agentic/agent:base" ;;
            dev)  echo "agentic/agent:dev" ;;
            *)    echo "agentic/${name}:${TAG}" ;;
        esac
    fi
}

build_one() {
    local name="$1"
    local tag
    tag="$(tag_of "${name}")"
    local dockerfile="images/container/Dockerfile.${name}"
    if [[ ! -f "${dockerfile}" ]]; then
        echo "build.sh: no Dockerfile for '${name}' (looked at ${dockerfile})" >&2
        return 1
    fi
    echo ">>> building ${tag} from ${dockerfile}"
    docker build -f "${dockerfile}" -t "${tag}" .
    if [[ ${PUSH} -eq 1 ]]; then
        echo ">>> pushing ${tag}"
        docker push "${tag}"
    fi
}

# Build order matters: base → dev → platforms (each platform FROMs dev).
# Build base first if requested, then dev (also if requested), then anything else.
if printf '%s\n' "${TARGETS[@]}" | grep -qx base; then
    build_one base
fi
if printf '%s\n' "${TARGETS[@]}" | grep -qx "${DEV_LAYER}"; then
    build_one "${DEV_LAYER}"
fi
for t in "${TARGETS[@]}"; do
    [[ "${t}" == "base" || "${t}" == "${DEV_LAYER}" ]] && continue
    build_one "${t}"
done

echo
echo "done. images:"
for t in "${TARGETS[@]}"; do
    tag_of "${t}"
done
