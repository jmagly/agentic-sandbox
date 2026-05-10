#!/usr/bin/env bash
# Test fixture replacing images/qemu/provision-vm.sh in admin_v2 unit tests.
# Echoes invocation args, sleeps briefly, exits 0 to simulate a successful
# provision. Used by the AIWG_PROVISION_VM_SCRIPT env var.
set -euo pipefail
echo "fake-provision-vm.sh invoked with args: $*"
sleep 0.05
exit 0
