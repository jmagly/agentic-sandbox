#!/usr/bin/env bash
# Tail admin logs over Server-Sent Events.
#
# operationId: streamLogs
# Path:        GET /api/v2/admin/logs?follow=true
# Auth:        bearer token (replace $ADMIN_TOKEN)
# Returns:     text/event-stream — one SSE event per LogEntry.
#              Heartbeat comments (": keepalive\n\n") every 15s.
#
# Each SSE record:
#   id: <monotonic-seq>
#   event: log
#   data: {"timestamp": "...", "level": "info", "target": "...", "message": "...", "fields": {...}}
#
# Surface 1 (admin/fleet) per ADR-022 — observability-semantic, but
# routed under the admin prefix and authenticated as an operator.

set -euo pipefail

HOST="${HOST:-localhost:8122}"
ADMIN_TOKEN="${ADMIN_TOKEN:?set ADMIN_TOKEN to the operator bearer token}"
LEVEL="${LEVEL:-info}"
TARGET="${TARGET:-}"

QUERY="follow=true&level=${LEVEL}"
if [[ -n "${TARGET}" ]]; then
  QUERY="${QUERY}&target=${TARGET}"
fi

# -N: disable curl output buffering so SSE events appear live.
# --no-buffer is the same flag spelled long-form.
exec curl -sS -N \
  "https://${HOST}/api/v2/admin/logs?${QUERY}" \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Accept: text/event-stream"
