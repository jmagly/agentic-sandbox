#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MANIFEST="$ROOT_DIR/setup.aiwg.yaml"
INSTALL_URL="https://raw.githubusercontent.com/jmagly/agentic-sandbox/main/setup.aiwg.yaml"

test -f "$MANIFEST"
grep -q '^apiVersion: setup.aiwg.io/v1$' "$MANIFEST"
grep -q '^kind: SetupManifest$' "$MANIFEST"
grep -q 'execution_mode: provider-orchestrated' "$MANIFEST"

for step in explain-and-discover audit-host-and-existing-installs propose-safe-plan \
  install-prerequisites-and-sandbox install-aiwg-and-cockpit start-and-verify handoff; do
  grep -q "id: $step" "$MANIFEST"
done

for safeguard in 'sudo or change group membership' 'unreviewed network response' \
  'copy secret values into project files' 'claim VM readiness'; do
  grep -qi "$safeguard" "$MANIFEST"
done

for doc in README.md docs/getting-started.md docs/aiwg-executor.md; do
  grep -q "$INSTALL_URL" "$ROOT_DIR/$doc"
done

echo "agentic setup manifest contract: PASS"
