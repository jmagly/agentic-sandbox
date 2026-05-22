#!/usr/bin/env bash
# Credential-free provider CLI inventory helper for sandbox automation-control loadouts.
# It checks binary presence and bounded --version output only; it does not start
# provider login, auth, device-code, or interactive task flows.
set -euo pipefail

timeout_s="${AGENTIC_PROVIDER_INVENTORY_TIMEOUT:-5}"
if [[ "$timeout_s" != *s ]]; then
  timeout_s="${timeout_s}s"
fi

if [[ $# -gt 0 ]]; then
  tools=("$@")
else
  tools=(codex claude opencode aider goose)
fi

printf 'schema\tagentic.provider_inventory.v1\n'
printf 'tool\tstatus\tversion\n'
for tool in "${tools[@]}"; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf '%s\tmissing\t\n' "$tool"
    continue
  fi
  version="$(timeout "$timeout_s" "$tool" --version 2>&1 | head -n 1 || true)"
  version="${version//$'\t'/ }"
  version="${version//$'\r'/ }"
  version="${version//$'\n'/ }"
  if [[ -z "$version" ]]; then
    version='present-version-empty'
  fi
  printf '%s\tpresent\t%s\n' "$tool" "$version"
done
