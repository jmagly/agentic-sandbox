#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="$SCRIPT_DIR/vfio-gpu-runner-guard.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

PASS=0
FAIL=0
pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }
assert_contains() {
    local label="$1" needle="$2" path="$3"
    if grep -qF -- "$needle" "$path"; then pass "$label"; else fail "$label"; fi
}
assert_not_contains() {
    local label="$1" needle="$2" path="$3"
    if grep -qF -- "$needle" "$path"; then fail "$label"; else pass "$label"; fi
}

pci="$TMP_ROOT/pci"
drm="$TMP_ROOT/drm"
dev="$TMP_ROOT/dev"
proc="$TMP_ROOT/proc"
dmi="$TMP_ROOT/dmi"
iommu="$TMP_ROOT/iommu_groups"
vms="$TMP_ROOT/vms"
mkdir -p "$pci/devices" "$pci/drivers/nvidia" "$pci/drivers/snd_hda_intel" \
    "$drm" "$dev/vfio" "$dev/dri" "$proc" "$dmi" "$iommu/23/devices" "$vms/.vfio-claims"

make_device() {
    local bdf="$1" class="$2" vendor="$3" device="$4" driver="$5"
    mkdir -p "$pci/devices/$bdf"
    printf '%s\n' "$class" > "$pci/devices/$bdf/class"
    printf '%s\n' "$vendor" > "$pci/devices/$bdf/vendor"
    printf '%s\n' "$device" > "$pci/devices/$bdf/device"
    ln -s "$pci/drivers/$driver" "$pci/devices/$bdf/driver"
}

make_device 0000:01:00.0 0x030000 0x10de 0x1abc nvidia
make_device 0000:41:00.0 0x030200 0x10de 0x2def nvidia
make_device 0000:41:00.1 0x040300 0x10de 0x22ef snd_hda_intel
touch "$pci/devices/0000:41:00.0/reset"
touch "$dev/dri/card-test"
printf 'flr bus\n' > "$pci/devices/0000:41:00.0/reset_method"
for bdf in 0000:41:00.0 0000:41:00.1; do
    ln -s "$iommu/23" "$pci/devices/$bdf/iommu_group"
    ln -s "$pci/devices/$bdf" "$iommu/23/devices/$bdf"
done
printf 'intel_iommu=on iommu=pt\n' > "$proc/cmdline"
printf 'ExampleVendor\n' > "$dmi/board_vendor"
printf 'ExampleBoard\n' > "$dmi/board_name"
printf 'ExampleFirmware\n' > "$dmi/bios_vendor"
printf '1.2.3\n' > "$dmi/bios_version"

config="$TMP_ROOT/runner.json"
cat > "$config" <<'EOF'
{
  "schema": 1,
  "host_class": "ci-dual-gpu-fixture",
  "repository": "roctinam/agentic-sandbox",
  "allowed_ref": "refs/heads/main",
  "allowed_actors": ["ci-admin"],
  "runner_label": "vfio-gpu",
  "lock_file": "/run/lock/agentic-sandbox-vfio-gpu.lock",
  "service_gpu": {"bdf": "0000:01:00.0", "native_driver": "nvidia"},
  "test_gpu": {
    "bdf": "0000:41:00.0",
    "iommu_group": "23",
    "group_members": ["0000:41:00.0", "0000:41:00.1"],
    "device_nodes": ["dri/card-test"],
    "native_drivers": {"0000:41:00.0": "nvidia", "0000:41:00.1": "snd_hda_intel"},
    "acs_reviewed": true
  }
}
EOF

run_guard() {
    AGENTIC_CH_PCI_SYSFS_ROOT="$pci" \
    AGENTIC_CH_DRM_CLASS_ROOT="$drm" \
    AGENTIC_CH_DEV_ROOT="$dev" \
    AGENTIC_CH_PROC_ROOT="$proc" \
    AGENTIC_DMI_ROOT="$dmi" \
    AGENTIC_IOMMU_GROUPS_ROOT="$iommu" \
    VM_STORAGE_DIR="$vms" \
        "$GUARD" "$@"
}

echo "=== Test: valid inventory and lifecycle guards ==="
if run_guard inventory --config "$config" | jq -e '.secrets_collected == false and .iommu_enabled == true and .test_gpu.iommu_group == "23"' >/dev/null; then
    pass "inventory records the approved dual-GPU host without secrets"
else
    fail "inventory records the approved dual-GPU host without secrets"
fi
if run_guard preflight --config "$config" | jq -e '.phase == "preflight" and .result == "pass"' >/dev/null; then
    pass "preflight accepts exact idle allowlisted group"
else
    fail "preflight accepts exact idle allowlisted group"
fi
if run_guard postflight --config "$config" --vm ci-vfio-tenant-a | jq -e '.phase == "postflight" and .result == "pass"' >/dev/null; then
    pass "postflight accepts restored drivers and clean state"
else
    fail "postflight accepts restored drivers and clean state"
fi

echo "=== Test: fail-closed policy ==="
bad_config="$TMP_ROOT/bad.json"
jq '.test_gpu.acs_reviewed=false' "$config" > "$bad_config"
if run_guard preflight --config "$bad_config" >/dev/null 2>&1; then
    fail "preflight rejects unreviewed ACS topology"
else
    pass "preflight rejects unreviewed ACS topology"
fi

mkdir -p "$vms/.vfio-claims/iommu-23"
if run_guard preflight --config "$config" >/dev/null 2>&1; then
    fail "preflight rejects an existing VFIO claim"
else
    pass "preflight rejects an existing VFIO claim"
fi
rmdir "$vms/.vfio-claims/iommu-23"

rm "$pci/devices/0000:01:00.0/driver"
ln -s "$pci/drivers/snd_hda_intel" "$pci/devices/0000:01:00.0/driver"
if run_guard preflight --config "$config" >/dev/null 2>&1; then
    fail "preflight rejects service GPU driver changes"
else
    pass "preflight rejects service GPU driver changes"
fi

echo "=== Test: restricted workflow and orchestration contract ==="
workflow="$SCRIPT_DIR/../.gitea/workflows/ci.yaml"
runner="$SCRIPT_DIR/run-vfio-gpu-validation.sh"
assert_contains "CI exposes manual dispatch" "workflow_dispatch:" "$workflow"
# shellcheck disable=SC2016 # Literal workflow expression is the assertion.
assert_contains "physical CI job is explicitly disabled" 'if: ${{ false }}' "$workflow"
assert_contains "disabled job uses a non-existent runner label" "runs-on: vfio-gpu-disabled" "$workflow"
assert_not_contains "physical job no longer targets Titan" "name: Exclusive dual-GPU VFIO validation on Titan" "$workflow"
assert_not_contains "workflow exposes no physical-GPU dispatch input" "vfio_gpu_confirmation" "$workflow"
assert_contains "disabled scaffold cannot satisfy local confirmation" "--confirmation CI-VFIO-DISABLED" "$workflow"
assert_contains "local entrypoint requires exact destructive confirmation" "RUN-LOCAL-VFIO" "$runner"
# shellcheck disable=SC2016 # Literal source fragment is the assertion.
assert_contains "local entrypoint rejects automated events" '[[ "$CI_EVENT" == "local_manual" ]]' "$runner"
assert_contains "runner holds a nonblocking global lock" "flock -n 9" "$runner"
# shellcheck disable=SC2016 # Literal source fragments are the assertions.
assert_contains "runner performs tenant-B pre-write residue probe" 'provision_tenant "$vm_b" probe-before-write' "$runner"
assert_contains "tenant-B verifier skips the driver probe" "verify_args+=(--skip-driver-probe)" "$runner"
assert_contains "tenant-B residue must attest pre-write timing" '.prewrite == true' "$runner"
# shellcheck disable=SC2016 # Literal source fragments are the assertions.
assert_contains "runner destroys through the managed backend" 'AGENTIC_BACKEND=cloud-hypervisor "$DESTROY_VM"' "$runner"
if "$GUARD" preflight --config "$SCRIPT_DIR/../configs/vfio-gpu-runner.example.json" >/dev/null 2>&1; then
    fail "placeholder inventory cannot authorize hardware operations"
else
    pass "placeholder inventory cannot authorize hardware operations"
fi
if jq -e '
    .host_class == "grissom-workstation-local-only" and
    .runner_label == "local-only" and
    .service_gpu.native_driver == "i915" and
    .test_gpu.acs_reviewed == false
' "$SCRIPT_DIR/../configs/vfio-gpu-runners/grissom.workstation-draft.json" >/dev/null; then
    pass "Grissom draft records local-only non-authorizing posture"
else
    fail "Grissom draft records local-only non-authorizing posture"
fi
if "$runner" --config "$config" --confirmation WRONG >/dev/null 2>&1; then
    fail "local entrypoint rejects incorrect destructive confirmation"
else
    pass "local entrypoint rejects incorrect destructive confirmation"
fi

echo ""
echo "=== Summary ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
(( FAIL == 0 ))
