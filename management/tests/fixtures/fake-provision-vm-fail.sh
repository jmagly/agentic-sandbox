#!/usr/bin/env bash
# Test fixture: simulates a failed provision-vm.sh run. Writes to stderr and
# exits 1 so admin_v2::provision_instance can verify failure handling and
# stderr capture in the operation result.
set -euo pipefail
echo "fake-provision-vm-fail.sh invoked with args: $*" >&2
echo "simulated provision failure: cloud-init timeout" >&2
sleep 0.05
exit 1
