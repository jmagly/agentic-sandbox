#!/usr/bin/env bash
# Test fixture that simulates a provisioner wedged past the admin-v2 watchdog.
set -euo pipefail
echo "fake-provision-vm-sleep.sh invoked with args: $*"
sleep "${FAKE_PROVISION_VM_SLEEP_SECONDS:-10}"
exit 0
