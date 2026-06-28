#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORKFLOW="$ROOT/.gitea/workflows/ci.yaml"
VERIFY="$ROOT/scripts/verify-release-assets.sh"

grep -F "needs: [release-attach]" "$WORKFLOW" >/dev/null \
  || { echo "GitHub release mirror must depend only on the canonical release attach job" >&2; exit 1; }

if grep -F "needs.release-binaries-mutsu.result == 'success'" "$WORKFLOW" >/dev/null; then
  echo "GitHub release mirror must not wait for the deferred mutsu Darwin lane" >&2
  exit 1
fi

grep -F "Darwin release artifacts are deferred" "$WORKFLOW" >/dev/null \
  || { echo "workflow must document that Darwin release artifacts are deferred" >&2; exit 1; }

if grep -F 'agentic-sandbox-${TAG}-aarch64-darwin.tar.gz' "$VERIFY" >/dev/null; then
  echo "release verifier must not require deferred Darwin artifacts on GitHub" >&2
  exit 1
fi

echo "GitHub release mirror asset test passed"
