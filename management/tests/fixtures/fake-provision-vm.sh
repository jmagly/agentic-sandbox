#!/usr/bin/env bash
# Test fixture replacing images/qemu/provision-vm.sh in admin_v2 unit tests.
# Echoes invocation args, sleeps briefly, exits 0 to simulate a successful
# provision. Used by the AIWG_PROVISION_VM_SCRIPT env var.
set -euo pipefail
echo "fake-provision-vm.sh invoked with args: $*"
if [[ -n "${AGENT_BOOTSTRAP_TOKEN:-}" ]]; then
  echo "bootstrap_token_env=set"
  echo "bootstrap_token_raw=$AGENT_BOOTSTRAP_TOKEN"
fi
if [[ -n "${AGENT_BOOTSTRAP_SPIFFE_ID:-}" ]]; then
    echo "bootstrap_spiffe_id=$AGENT_BOOTSTRAP_SPIFFE_ID"
fi
if [[ -n "${AGENT_BOOTSTRAP_ENROLLMENT_URL:-}" ]]; then
    echo "bootstrap_enrollment_url=$AGENT_BOOTSTRAP_ENROLLMENT_URL"
fi
sleep 0.05
exit 0
