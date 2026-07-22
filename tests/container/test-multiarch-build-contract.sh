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

grep -q 'FROM rust:1.88-bookworm@sha256:' images/container/Dockerfile.base
grep -q 'COPY --from=agent-builder' images/container/Dockerfile.base
grep -q 'codex-linux-arm64' images/container/Dockerfile.codex
grep -qx '\*\*/target' .dockerignore
echo "multiarch image contract: ok"
