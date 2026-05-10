#!/usr/bin/env bash
# Restart a provisioned instance (async).
#
# operationId: restartInstance
# Path:        POST /api/v2/admin/instances/{id}/restart
# Auth:        bearer token (replace $ADMIN_TOKEN)
# Returns:     202 Accepted with an OperationStatus body.
#
# Surface 1 (admin/fleet) per ADR-022 — NOT A2A.

set -euo pipefail

HOST="${HOST:-localhost:8122}"
ADMIN_TOKEN="${ADMIN_TOKEN:?set ADMIN_TOKEN to the operator bearer token}"
INSTANCE_ID="${1:-${INSTANCE_ID:?pass instance id as first arg or set INSTANCE_ID}}"

# Kick off the restart.
RESPONSE="$(
  curl -sS -X POST \
    "https://${HOST}/api/v2/admin/instances/${INSTANCE_ID}/restart" \
    -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    -H "Accept: application/json"
)"

echo "Restart accepted:"
echo "${RESPONSE}"

# Extract operation id and poll for terminal state.
OP_ID="$(printf '%s' "${RESPONSE}" | jq -r '.id')"

while :; do
  STATUS="$(
    curl -sS \
      "https://${HOST}/api/v2/admin/operations/${OP_ID}" \
      -H "Authorization: Bearer ${ADMIN_TOKEN}" \
      -H "Accept: application/json"
  )"
  STATE="$(printf '%s' "${STATUS}" | jq -r '.state')"
  printf 'op=%s state=%s\n' "${OP_ID}" "${STATE}"
  case "${STATE}" in
    succeeded|failed|canceled) break ;;
  esac
  sleep 2
done

printf '%s\n' "${STATUS}" | jq .
