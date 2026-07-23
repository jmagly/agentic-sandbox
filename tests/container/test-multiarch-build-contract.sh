#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

output="$(scripts/build-multiarch-agent-images.sh \
  --registry registry.example.test/agentic-sandbox \
  --revision test-sha \
  --release-tag v-test \
  --dry-run)"

grep -q -- '--platform linux/amd64,linux/arm64' <<<"$output"
grep -q 'agent:base-test-sha' <<<"$output"
grep -q 'agent:dev-test-sha' <<<"$output"
grep -q 'AGENT_BASE_IMAGE=registry.example.test/agentic-sandbox/agent:base-test-sha' <<<"$output"
grep -q 'AGENT_DEV_IMAGE=registry.example.test/agentic-sandbox/agent:dev-test-sha' <<<"$output"
grep -q 'CODEX_IMAGE=registry.example.test/agentic-sandbox/codex:test-sha' <<<"$output"
[[ "$(grep -c 'docker buildx build' <<<"$output")" -eq 6 ]]
grep -q 'inspect_with_retry' scripts/build-multiarch-agent-images.sh
grep -q 'AGENT_IMAGE_INSPECT_ATTEMPTS' scripts/build-multiarch-agent-images.sh
grep -q 'AGENT_IMAGE_INSPECT_DELAY_SECONDS' scripts/build-multiarch-agent-images.sh

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
mkdir -p "$tmp_dir/bin" "$tmp_dir/state"
ln -s "$repo_root/tests/container/fake-docker-imagetools.sh" "$tmp_dir/bin/docker"
PATH="$tmp_dir/bin:$PATH" \
  FAKE_DOCKER_STATE="$tmp_dir/state" \
  AGENT_IMAGE_INSPECT_ATTEMPTS=2 \
  AGENT_IMAGE_INSPECT_DELAY_SECONDS=0 \
  scripts/build-multiarch-agent-images.sh \
    --registry registry.example.test/agentic-sandbox \
    --revision retry-test \
    --evidence "$tmp_dir/evidence.jsonl"
[[ "$(wc -l < "$tmp_dir/evidence.jsonl")" -eq 6 ]]
[[ "$(find "$tmp_dir/state" -type f | wc -l)" -eq 12 ]]

grep -q 'FROM rust:1.88-bookworm@sha256:' images/container/Dockerfile.base
grep -q 'COPY --from=agent-builder' images/container/Dockerfile.base
grep -q 'codex-linux-arm64' images/container/Dockerfile.codex
grep -qx 'ARG TARGETARCH' images/container/Dockerfile.dev
grep -qx 'ARG TARGETARCH' images/container/Dockerfile.codex
grep -q 'amd64) rustarch=x86_64; grpcarch=x86_64 ;;' images/container/Dockerfile.dev
grep -q 'arm64) rustarch=aarch64; grpcarch=arm64 ;;' images/container/Dockerfile.dev
if grep -q '^ARG TARGETARCH=' images/container/Dockerfile.dev images/container/Dockerfile.codex; then
  echo "TARGETARCH must inherit BuildKit's automatic per-platform value" >&2
  exit 1
fi
grep -qx '\*\*/target' .dockerignore
echo "multiarch image contract: ok"
