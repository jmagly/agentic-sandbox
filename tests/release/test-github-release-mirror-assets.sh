#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORKFLOW="$ROOT/.gitea/workflows/ci.yaml"
VERIFY="$ROOT/scripts/verify-release-assets.sh"

grep -F "needs: [release-attach]" "$WORKFLOW" >/dev/null \
  || { echo "GitHub release mirror must depend only on the canonical release attach job" >&2; exit 1; }

mirror_block="$(
  sed -n '/^  github-release-sync:/,$p' "$WORKFLOW"
)"
grep -Fq 'fetch-depth: 0' <<<"$mirror_block" \
  || { echo "GitHub release mirror must fetch exact tag history" >&2; exit 1; }
grep -Fq 'gh auth setup-git' <<<"$mirror_block" \
  || { echo "GitHub mirror must use the credential helper instead of token-bearing URLs" >&2; exit 1; }
grep -Fq '"$GITHUB_SHA:refs/heads/main"' <<<"$mirror_block" \
  || { echo "GitHub mirror must synchronize the exact release commit" >&2; exit 1; }
grep -Fq '"refs/tags/${TAG_NAME}:refs/tags/${TAG_NAME}"' <<<"$mirror_block" \
  || { echo "GitHub mirror must synchronize the immutable release tag" >&2; exit 1; }
grep -Fq 'GitHub main diverged from the Gitea release history' <<<"$mirror_block" \
  || { echo "GitHub mirror must fail closed on divergent history" >&2; exit 1; }
if grep -Eq 'https://[^/]*\\$\\{?GH_TOKEN|x-access-token' <<<"$mirror_block"; then
  echo "GitHub mirror must not put credentials in its remote URL" >&2
  exit 1
fi

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
