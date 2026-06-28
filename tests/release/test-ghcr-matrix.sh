#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORKFLOW="$ROOT/.gitea/workflows/ci.yaml"
RUNBOOK="$ROOT/docs/releases/runbook.md"
RELEASE_NOTE="$ROOT/docs/releases/v2026.6.2.md"

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

grep -F 'docker pull ghcr.io/<owner>/${image}:v<version>' "$RUNBOOK" >/dev/null \
  || { echo "missing runbook GHCR pull loop" >&2; exit 1; }

grep -F "GHCR_TOKEN is required for public release container publication (#478)" "$WORKFLOW" >/dev/null \
  || { echo "GHCR_TOKEN release-blocking error is missing" >&2; exit 1; }

grep -F "Smoke-test public GHCR release images" "$WORKFLOW" >/dev/null \
  || { echo "public GHCR smoke check step is missing" >&2; exit 1; }

grep -F "docker logout ghcr.io || true" "$WORKFLOW" >/dev/null \
  || { echo "GHCR smoke check must prove anonymous public pulls" >&2; exit 1; }

grep -F "docker run --rm --entrypoint /bin/sh \"\$MGMT_REF\"" "$WORKFLOW" >/dev/null \
  || { echo "GHCR management image smoke check is missing" >&2; exit 1; }

grep -F "docker run --rm --entrypoint /usr/local/bin/agent-client \"\$AGENT_REF\" --help" "$WORKFLOW" >/dev/null \
  || { echo "GHCR agent-client image smoke check is missing" >&2; exit 1; }

grep -F "image-digests.txt" "$WORKFLOW" >/dev/null \
  || { echo "GHCR digest output is missing" >&2; exit 1; }

grep -F "ghcr.io/<owner>/agentic-sandbox-mgmt:v<version>" "$RUNBOOK" >/dev/null \
  || { echo "runbook compose example is missing GHCR management image" >&2; exit 1; }

echo "GHCR release matrix test passed"
