#!/usr/bin/env bash
# shellcheck disable=SC2016 # Match literal workflow expressions and shell variables.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORKFLOW="$ROOT/.gitea/workflows/notify-site.yml"
SPEC="$ROOT/ci/vault-fetch.notify-site.spec"

grep -Fq 'tags:' "$WORKFLOW" \
  || { echo "site notification must run for release tags" >&2; exit 1; }
grep -Fq 'git show "${REF_NAME}:setup.aiwg.yaml"' "$WORKFLOW" \
  || { echo "site notification must publish manifest bytes from the selected release tag" >&2; exit 1; }
grep -Fq 'SETUP_SHA256=' "$WORKFLOW" \
  || { echo "site notification must bind publication to a manifest digest" >&2; exit 1; }
grep -Fq 'agentic_sandbox_source_tag' "$WORKFLOW" \
  || { echo "site notification must send the immutable source tag" >&2; exit 1; }
grep -Fq 'agentic_sandbox_setup_sha256' "$WORKFLOW" \
  || { echo "site notification must send the expected manifest digest" >&2; exit 1; }
grep -Fq 'bash ci/vault-fetch.sh --spec ci/vault-fetch.notify-site.spec' "$WORKFLOW" \
  || { echo "site notification must fetch its token through the reviewed vault spec" >&2; exit 1; }
grep -Fq 'env -u AIWG_IO_DISPATCH_TOKEN curl --config /dev/fd/3' "$WORKFLOW" \
  || { echo "site notification must remove the token from the curl environment and argv" >&2; exit 1; }
grep -Fq '3<<<"header = \"Authorization: token ${AIWG_IO_DISPATCH_TOKEN}\""' "$WORKFLOW" \
  || { echo "site notification must pass authorization over a private file descriptor" >&2; exit 1; }

if grep -Eq -- '-H "Authorization: token \$\{AIWG_IO_DISPATCH_TOKEN\}"' "$WORKFLOW"; then
  echo "site notification must not expose its token in curl process arguments" >&2
  exit 1
fi

VAULT_ADDR='https://vault.example.test' \
AIWG_IO_DISPATCH_TOKEN_VAULT_PATH='secret/data/ci/aiwg-io' \
AIWG_IO_DISPATCH_TOKEN_VAULT_FIELD='token' \
  bash "$ROOT/ci/vault-fetch.sh" --spec "$SPEC" --dry-run >/dev/null

echo "aiwg.io release notification contract passed"
