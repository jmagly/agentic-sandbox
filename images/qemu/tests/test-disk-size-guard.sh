#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TMP_ROOT="$(mktemp -d /tmp/agentic-disk-size-guard.XXXXXX)"
trap 'rm -rf "$TMP_ROOT"' EXIT

# shellcheck source=../lib/resources.sh
source "$ROOT_DIR/images/qemu/lib/resources.sh"
# shellcheck source=../cloud-init/common.sh
source "$ROOT_DIR/images/qemu/cloud-init/common.sh"

qemu-img create -q -f qcow2 "$TMP_ROOT/base.qcow2" 40M
export AIWG_SKIP_BASE_VERIFY=1

if create_overlay_disk "$TMP_ROOT/base.qcow2" "$TMP_ROOT/small-overlay.qcow2" 8M \
    >"$TMP_ROOT/small-overlay.log" 2>&1; then
  echo "FAIL: overlay helper accepted a disk smaller than its base" >&2
  exit 1
fi
test ! -e "$TMP_ROOT/small-overlay.qcow2"
grep -Fq 'smaller than base virtual size' "$TMP_ROOT/small-overlay.log"

create_overlay_disk "$TMP_ROOT/base.qcow2" "$TMP_ROOT/large-overlay.qcow2" 48M \
  >"$TMP_ROOT/large-overlay.log" 2>&1
test "$(qemu-img info --output=json "$TMP_ROOT/large-overlay.qcow2" | jq -r '."virtual-size"')" \
  -eq 50331648

if create_standalone_disk "$TMP_ROOT/base.qcow2" "$TMP_ROOT/small-standalone.qcow2" 8M \
    >"$TMP_ROOT/small-standalone.log" 2>&1; then
  echo "FAIL: standalone helper accepted a disk smaller than its base" >&2
  exit 1
fi
test ! -e "$TMP_ROOT/small-standalone.qcow2"
grep -Fq 'smaller than base virtual size' "$TMP_ROOT/small-standalone.log"

echo "disk creation rejects requested sizes below the base virtual size"
