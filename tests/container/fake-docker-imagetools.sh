#!/usr/bin/env bash
set -euo pipefail

state_dir="${FAKE_DOCKER_STATE:?FAKE_DOCKER_STATE is required}"

if [[ "${1:-}" == "buildx" && "${2:-}" == "build" ]]; then
  exit 0
fi

if [[ "${1:-}" != "buildx" || "${2:-}" != "imagetools" || "${3:-}" != "inspect" ]]; then
  echo "unexpected fake docker invocation: $*" >&2
  exit 2
fi

mode=summary
if [[ "${4:-}" == "--raw" ]]; then
  mode=raw
fi
ref="${*: -1}"
key="$(printf '%s' "$mode:$ref" | sha256sum | awk '{print $1}')"
attempt_file="$state_dir/$key"

if [[ ! -e "$attempt_file" ]]; then
  : > "$attempt_file"
  echo "simulated registry propagation delay" >&2
  exit 255
fi

if [[ "$mode" == "raw" ]]; then
  printf '%s\n' '{"manifests":[{"digest":"sha256:amd64","platform":{"os":"linux","architecture":"amd64"}},{"digest":"sha256:arm64","platform":{"os":"linux","architecture":"arm64"}}]}'
else
  printf '%s\n' 'Name: test' 'Digest: sha256:index'
fi
