#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORKFLOW="$ROOT/.gitea/workflows/ci.yaml"
RUNBOOK="$ROOT/docs/releases/runbook.md"
RELEASE_NOTE="$ROOT/docs/releases/v2026.6.2.md"
VERIFIER="$ROOT/scripts/verify-release-assets.sh"

required_pairs=(
  "agentic-mgmt|agentic-sandbox-mgmt"
  "agent-client|agentic-sandbox-agent-client"
  "agent|agentic-sandbox-agent"
  "claude|agentic-sandbox-claude"
  "codex|agentic-sandbox-codex"
  "opencode|agentic-sandbox-opencode"
  "automation-control|agentic-sandbox-automation-control"
)

for pair in "${required_pairs[@]}"; do
  internal="${pair%%|*}"
  public="${pair##*|}"

  grep -F "${internal}|${public}" "$WORKFLOW" >/dev/null \
    || { echo "missing workflow GHCR image mapping: ${pair}" >&2; exit 1; }

  grep -F "${public}" "$RUNBOOK" >/dev/null \
    || { echo "missing runbook GHCR image name: ${public}" >&2; exit 1; }

  grep -F "ghcr.io/<owner>/${public}:v2026.6.2" "$RELEASE_NOTE" >/dev/null \
    || { echo "missing release-note GHCR pull example: ${public}" >&2; exit 1; }
done

# shellcheck disable=SC2016 # The runbook example must retain literal ${image}.
grep -F 'docker pull ghcr.io/<owner>/${image}:v<version>' "$RUNBOOK" >/dev/null \
  || { echo "missing runbook GHCR pull loop" >&2; exit 1; }

grep -F "required to fetch the GHCR token from vault for public release publication (#478)" "$WORKFLOW" >/dev/null \
  || { echo "vault-backed GHCR release-blocking error is missing" >&2; exit 1; }

grep -F "Smoke-test public GHCR release images" "$WORKFLOW" >/dev/null \
  || { echo "public GHCR smoke check step is missing" >&2; exit 1; }

grep -F "docker logout ghcr.io || true" "$WORKFLOW" >/dev/null \
  || { echo "GHCR smoke check must prove anonymous public pulls" >&2; exit 1; }

grep -F "docker run --rm --entrypoint /bin/sh \"\$MGMT_REF\"" "$WORKFLOW" >/dev/null \
  || { echo "GHCR management image smoke check is missing" >&2; exit 1; }

grep -F "docker run --rm --entrypoint /usr/local/bin/agent-client \"\$AGENT_REF\" --help" "$WORKFLOW" >/dev/null \
  || { echo "GHCR agent-client image smoke check is missing" >&2; exit 1; }

grep -F "public-image-digests.jsonl" "$WORKFLOW" >/dev/null \
  || { echo "GHCR digest output is missing" >&2; exit 1; }

grep -F "scripts/mirror-oci-index.sh" "$WORKFLOW" >/dev/null \
  || { echo "OCI index-preserving mirror helper is missing" >&2; exit 1; }

grep -F -- "--required-platform linux/arm64" "$WORKFLOW" >/dev/null \
  || { echo "public provider mirrors do not require linux/arm64" >&2; exit 1; }

# shellcheck disable=SC2016 # Match literal verifier variables.
grep -F 'docker buildx imagetools inspect --raw "$ref"' "$VERIFIER" >/dev/null \
  || { echo "release verifier does not inspect public provider OCI indexes" >&2; exit 1; }

# shellcheck disable=SC2016 # Match literal verifier variables.
grep -F 'anonymous_docker_config="${TMPDIR_RELEASE}/docker-anonymous"' "$VERIFIER" >/dev/null \
  || { echo "release verifier does not isolate public pulls from stored registry credentials" >&2; exit 1; }

# shellcheck disable=SC2016 # Match literal verifier variables.
grep -F 'export DOCKER_CONFIG="$anonymous_docker_config"' "$VERIFIER" >/dev/null \
  || { echo "release verifier does not use its anonymous Docker configuration" >&2; exit 1; }

grep -F '.platform.architecture == "amd64"' "$VERIFIER" >/dev/null \
  || { echo "release verifier does not require linux/amd64 provider manifests" >&2; exit 1; }

grep -F '.platform.architecture == "arm64"' "$VERIFIER" >/dev/null \
  || { echo "release verifier does not require linux/arm64 provider manifests" >&2; exit 1; }

# shellcheck disable=SC2016 # Match literal verifier variables.
grep -F 'for image in "${provider_images[@]}"' "$VERIFIER" >/dev/null \
  || { echo "release verifier does not iterate over the provider matrix" >&2; exit 1; }

# shellcheck disable=SC2016 # Match literal verifier variables.
grep -F 'docker run --rm --user 10001:10001 --entrypoint /bin/sh "$ref"' "$VERIFIER" >/dev/null \
  || { echo "release verifier does not smoke-test provider runtime identity" >&2; exit 1; }

# shellcheck disable=SC2016 # Match literal verifier variables.
grep -F 'state_dir="$HOME/.local/state/agentic-sandbox/grpc-mtls"' "$VERIFIER" >/dev/null \
  || { echo "release verifier does not smoke-test writable provider runtime state" >&2; exit 1; }

# shellcheck disable=SC2016 # Match literal workflow variables.
if grep -F 'docker tag "$SRC" "$DST"' "$WORKFLOW" >/dev/null ||
   grep -F 'docker push "$DST"' "$WORKFLOW" >/dev/null; then
  echo "GHCR mirror must not collapse an OCI index through pull/tag/push" >&2
  exit 1
fi

grep -F "ghcr.io/<owner>/agentic-sandbox-mgmt:v<version>" "$RUNBOOK" >/dev/null \
  || { echo "runbook compose example is missing GHCR management image" >&2; exit 1; }

echo "GHCR release matrix test passed"
