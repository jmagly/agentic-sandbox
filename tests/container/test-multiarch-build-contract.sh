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
grep -q -- '--tag registry.example.test/agentic-sandbox/agent:test-sha' <<<"$output"
grep -q -- '--tag registry.example.test/agentic-sandbox/agent:v-test' <<<"$output"
[[ "$(grep -o -- '--tag registry.example.test/agentic-sandbox/agent:test-sha' <<<"$output" | wc -l)" -eq 1 ]]
[[ "$(grep -o -- '--tag registry.example.test/agentic-sandbox/agent:v-test' <<<"$output" | wc -l)" -eq 1 ]]
grep -q 'AGENT_BASE_IMAGE=registry.example.test/agentic-sandbox/agent:base-test-sha' <<<"$output"
grep -q 'AGENT_DEV_IMAGE=registry.example.test/agentic-sandbox/agent:dev-test-sha' <<<"$output"
grep -q 'CODEX_IMAGE=registry.example.test/agentic-sandbox/codex:test-sha' <<<"$output"
grep -q -- '--build-arg CARGO_BUILD_JOBS=8' <<<"$output"
[[ "$(grep -c 'docker buildx build' <<<"$output")" -eq 6 ]]
grep -q 'inspect_with_retry' scripts/build-multiarch-agent-images.sh
grep -q 'AGENT_IMAGE_INSPECT_ATTEMPTS' scripts/build-multiarch-agent-images.sh
grep -q 'AGENT_IMAGE_INSPECT_DELAY_SECONDS' scripts/build-multiarch-agent-images.sh
if AGENT_IMAGE_CARGO_BUILD_JOBS=0 scripts/build-multiarch-agent-images.sh \
  --registry registry.example.test/agentic-sandbox --dry-run >/dev/null 2>&1; then
  echo "zero Cargo Docker job budget must fail closed" >&2
  exit 1
fi

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
grep -Fqx 'COPY .cargo/config.toml .cargo/config.toml' images/container/Dockerfile.base
for dockerfile in Dockerfile.dev deploy/docker/Dockerfile.management deploy/docker/Dockerfile.agent-rust images/container/Dockerfile.base; do
  grep -Fqx 'ARG CARGO_BUILD_JOBS=8' "$dockerfile"
  grep -F 'case "$CARGO_BUILD_JOBS" in' "$dockerfile" >/dev/null
  grep -F 'cargo build --jobs "$CARGO_BUILD_JOBS"' "$dockerfile" >/dev/null
done
grep -F 'CARGO_DOCKER_BUILD_JOBS: "8"' .gitea/workflows/ci.yaml >/dev/null
[[ "$(grep -c -- '--build-arg.*CARGO_BUILD_JOBS' .gitea/workflows/ci.yaml)" -ge 2 ]]
[[ "$(grep -c -- 'timeout --signal=TERM --kill-after=60s 45m' .gitea/workflows/ci.yaml)" -ge 2 ]]
grep -F -- '--provenance=false' .gitea/workflows/ci.yaml >/dev/null
grep -F -- '--sbom=false' .gitea/workflows/ci.yaml >/dev/null
grep -F 'CARGO_BUILD_JOBS=${CARGO_DOCKER_BUILD_JOBS}' .gitea/workflows/ci.yaml >/dev/null
grep -F 'AGENT_IMAGE_CARGO_BUILD_JOBS:-8' scripts/build-multiarch-agent-images.sh >/dev/null
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
