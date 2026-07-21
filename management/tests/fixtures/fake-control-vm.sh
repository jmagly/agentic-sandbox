#!/usr/bin/env bash
set -euo pipefail

printf 'provider=%s action=%s vm=%s\n' \
    "${AGENTIC_BACKEND:?AGENTIC_BACKEND is required}" \
    "${1:?action is required}" \
    "${2:?vm name is required}" \
    >> "${FAKE_VM_CONTROL_LOG:?FAKE_VM_CONTROL_LOG is required}"
