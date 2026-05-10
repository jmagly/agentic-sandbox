#!/usr/bin/env bash
# Provision a new instance (async).
#
# operationId: provisionInstance
# Path:        POST /api/v2/admin/instances
# Auth:        bearer token (replace $ADMIN_TOKEN)
# Returns:     202 Accepted with an OperationStatus body. Poll
#              /api/v2/admin/operations/{id} until state is terminal.
#
# Surface 1 (admin/fleet) per ADR-022 — NOT A2A.

set -euo pipefail

HOST="${HOST:-localhost:8122}"
ADMIN_TOKEN="${ADMIN_TOKEN:?set ADMIN_TOKEN to the operator bearer token}"

curl -sS -X POST \
  "https://${HOST}/api/v2/admin/instances" \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json" \
  --data @- <<'JSON'
{
  "name": "agent-03",
  "runtime": "qemu",
  "loadout": "profiles/claude-only.yaml",
  "agentshare": true,
  "start": true,
  "labels": {
    "owner": "ops",
    "purpose": "experiment"
  }
}
JSON
