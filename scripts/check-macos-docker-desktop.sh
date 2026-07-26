#!/usr/bin/env bash
# Read-only, sanitized Docker Desktop readiness diagnostic for mutsu.

set -euo pipefail

docker_bin="${AGENTIC_DOCKER_BIN:-docker}"

if ! command -v "$docker_bin" >/dev/null 2>&1; then
  echo "status=not-ready diagnostic=cli-missing"
  exit 20
fi

if ! active_context="$("$docker_bin" context show 2>/dev/null)" ||
   [[ -z "$active_context" ]]; then
  echo "status=not-ready diagnostic=context-inactive"
  exit 21
fi

context_allowed=false
IFS=',' read -r -a allowed_contexts <<<"${AGENTIC_DOCKER_DESKTOP_CONTEXTS:-desktop-linux,default}"
for allowed_context in "${allowed_contexts[@]}"; do
  if [[ "$active_context" == "$allowed_context" ]]; then
    context_allowed=true
    break
  fi
done
if [[ "$context_allowed" != true ]]; then
  echo "status=not-ready diagnostic=context-inactive"
  exit 21
fi

if ! daemon_profile="$("$docker_bin" info --format '{{.OSType}} {{.Architecture}}' 2>/dev/null)"; then
  echo "status=not-ready diagnostic=daemon-unreachable"
  exit 22
fi

read -r docker_os docker_arch <<<"$daemon_profile"
if [[ "$docker_os" != "linux" || "$docker_arch" != "aarch64" ]]; then
  echo "status=not-ready diagnostic=context-inactive"
  exit 21
fi

echo "status=ready diagnostic=none docker_os=linux docker_arch=aarch64"
