#!/usr/bin/env bash
# Fail-closed inventory and lifecycle guards for local physical VFIO validation.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: vfio-gpu-runner-guard.sh COMMAND --config PATH [--vm NAME]

Commands:
  inventory   Emit non-secret hardware/software inventory JSON.
  preflight   Prove the service GPU is preserved and the test group is idle.
  postflight  Prove native drivers and host state were restored after --vm.
EOF
}

COMMAND="${1:-}"
[[ -n "$COMMAND" ]] || { usage >&2; exit 2; }
shift
CONFIG=""
VM_NAME=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --config) CONFIG="${2:?--config requires a path}"; shift 2 ;;
        --vm) VM_NAME="${2:?--vm requires a name}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

[[ "$COMMAND" =~ ^(inventory|preflight|postflight)$ ]] \
    || { echo "Unknown command: $COMMAND" >&2; usage >&2; exit 2; }
[[ -f "$CONFIG" ]] || { echo "VFIO inventory is missing: $CONFIG" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }

PCI_ROOT="${AGENTIC_CH_PCI_SYSFS_ROOT:-/sys/bus/pci}"
DRM_ROOT="${AGENTIC_CH_DRM_CLASS_ROOT:-/sys/class/drm}"
DEV_ROOT="${AGENTIC_CH_DEV_ROOT:-/dev}"
PROC_ROOT="${AGENTIC_CH_PROC_ROOT:-/proc}"
VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
CLAIM_ROOT="${AGENTIC_CH_VFIO_CLAIM_ROOT:-$VM_STORAGE_DIR/.vfio-claims}"

die() { echo "VFIO runner guard: $*" >&2; exit 1; }

valid_bdf() { [[ "$1" =~ ^[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-7]$ ]]; }
pci_driver() {
    local link="$PCI_ROOT/devices/$1/driver"
    [[ -L "$link" ]] && basename "$(readlink -f "$link")"
}
read_trimmed() {
    local path="$1"
    if [[ -r "$path" ]]; then
        tr '\n' ' ' < "$path" | sed 's/[[:space:]]*$//'
    fi
}

jq -e '
  .schema == 1 and
  (.host_class | type == "string" and length > 0 and . != "REPLACE_ME") and
  (.repository | type == "string" and length > 0) and
  (.allowed_ref | type == "string" and startswith("refs/heads/")) and
  (.allowed_actors | type == "array" and length > 0 and all(.[]; type == "string" and length > 0 and . != "REPLACE_ME")) and
  (.runner_label | type == "string" and length > 0) and
  (.lock_file | type == "string" and startswith("/run/lock/") and length > 10) and
  (.service_gpu.bdf | type == "string") and
  (.service_gpu.native_driver | type == "string" and length > 0) and
  (.test_gpu.bdf | type == "string") and
  (.test_gpu.iommu_group | type == "string" and test("^[0-9]+$")) and
  (.test_gpu.group_members | type == "array" and length > 0) and
  (.test_gpu.native_drivers | type == "object") and
  (.test_gpu.device_nodes | type == "array" and length > 0 and all(.[]; type == "string" and test("^[A-Za-z0-9._/-]+$") and (startswith("/") | not) and (contains("..") | not))) and
  .test_gpu.acs_reviewed == true
' "$CONFIG" >/dev/null || die "inventory schema, actor allowlist, or ACS approval is invalid"

service_bdf="$(jq -r '.service_gpu.bdf | ascii_downcase' "$CONFIG")"
test_bdf="$(jq -r '.test_gpu.bdf | ascii_downcase' "$CONFIG")"
valid_bdf "$service_bdf" || die "invalid service GPU BDF: $service_bdf"
valid_bdf "$test_bdf" || die "invalid test GPU BDF: $test_bdf"
[[ "$service_bdf" != "$test_bdf" ]] || die "service and test GPU must be distinct"

service_dir="$PCI_ROOT/devices/$service_bdf"
test_dir="$PCI_ROOT/devices/$test_bdf"
[[ -d "$service_dir" && -d "$test_dir" ]] || die "configured GPU is absent from PCI sysfs"
[[ "$(read_trimmed "$service_dir/class")" =~ ^0x03[0-9A-Fa-f]{4}$ ]] \
    || die "service GPU is not display class"
[[ "$(read_trimmed "$test_dir/class")" =~ ^0x03[0-9A-Fa-f]{4}$ ]] \
    || die "test GPU is not display class"
[[ -L "$test_dir/iommu_group" ]] || die "test GPU has no IOMMU group"

group_dir="$(readlink -f "$test_dir/iommu_group")"
group_id="$(basename "$group_dir")"
expected_group="$(jq -r '.test_gpu.iommu_group' "$CONFIG")"
[[ "$group_id" == "$expected_group" ]] \
    || die "test GPU moved from approved IOMMU group $expected_group to $group_id"
mapfile -t actual_members < <(find "$group_dir/devices" -mindepth 1 -maxdepth 1 -printf '%f\n' | sort)
mapfile -t approved_members < <(jq -r '.test_gpu.group_members[] | ascii_downcase' "$CONFIG" | sort)
[[ "$(printf '%s\n' "${actual_members[@]}")" == "$(printf '%s\n' "${approved_members[@]}")" ]] \
    || die "actual IOMMU group membership differs from the committed allowlist"
printf '%s\n' "${approved_members[@]}" | grep -Fxq "$test_bdf" \
    || die "test GPU is absent from its approved group member list"
[[ -e "$test_dir/reset" ]] || die "test GPU has no sysfs reset interface"

assert_service_gpu_preserved() {
    local expected actual
    expected="$(jq -r '.service_gpu.native_driver' "$CONFIG")"
    actual="$(pci_driver "$service_bdf")"
    [[ "$actual" == "$expected" ]] \
        || die "service GPU driver changed: expected $expected, got ${actual:-none}"
}

assert_native_test_drivers() {
    local member expected actual
    for member in "${approved_members[@]}"; do
        expected="$(jq -r --arg bdf "$member" '.test_gpu.native_drivers[$bdf] // empty' "$CONFIG")"
        [[ -n "$expected" ]] || die "no approved native driver for IOMMU member $member"
        actual="$(pci_driver "$member")"
        [[ "$actual" == "$expected" ]] \
            || die "native driver not restored for $member: expected $expected, got ${actual:-none}"
    done
}

assert_test_gpu_idle() {
    local entry name entry_bdf node node_rel probe_rc output
    local -a nodes=()
    while IFS= read -r node_rel; do
        node="$DEV_ROOT/$node_rel"
        [[ -e "$node" ]] || die "approved test GPU device node is missing: $node"
        nodes+=("$node")
    done < <(jq -r '.test_gpu.device_nodes[]' "$CONFIG")
    for entry in "$DRM_ROOT"/card* "$DRM_ROOT"/renderD*; do
        [[ -e "$entry/device" ]] || continue
        name="$(basename "$entry")"
        [[ "$name" =~ ^(card|renderD)[0-9]+$ ]] || continue
        entry_bdf="$(basename "$(readlink -f "$entry/device")")"
        if [[ "$entry_bdf" == "$test_bdf" && -e "$DEV_ROOT/dri/$name" ]]; then
            nodes+=("$DEV_ROOT/dri/$name")
        fi
    done
    command -v fuser >/dev/null 2>&1 || die "fuser is required to prove the test GPU is idle"
    while IFS= read -r node; do
        if output="$(fuser -s "$node" 2>&1)"; then
            die "test GPU is active through $node"
        else
            probe_rc=$?
            [[ "$probe_rc" -eq 1 && -z "$output" ]] \
                || die "could not prove the test GPU is idle through $node"
        fi
    done < <(printf '%s\n' "${nodes[@]}" | sort -u)
}

assert_no_claim() {
    [[ ! -e "$CLAIM_ROOT/iommu-$group_id" ]] \
        || die "IOMMU group $group_id still has a durable VFIO claim"
}

assert_no_stale_vmm() {
    local vm="$1" pid_file pid
    [[ "$vm" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]] || die "unsafe VM name"
    pid_file="$VM_STORAGE_DIR/$vm/cloud-hypervisor/pid"
    if [[ -f "$pid_file" ]]; then
        pid="$(cat "$pid_file" 2>/dev/null || true)"
        [[ -z "$pid" || ! -d "$PROC_ROOT/$pid" ]] \
            || die "Cloud Hypervisor process $pid remains for $vm"
    fi
    [[ ! -d "$VM_STORAGE_DIR/$vm" ]] || die "VM state directory remains after managed teardown: $vm"
}

emit_inventory() {
    local service_driver test_driver reset_method iommu_enabled
    service_driver="$(pci_driver "$service_bdf")"
    test_driver="$(pci_driver "$test_bdf")"
    reset_method="$(read_trimmed "$test_dir/reset_method")"
    iommu_enabled=false
    if [[ -d "${AGENTIC_IOMMU_GROUPS_ROOT:-/sys/kernel/iommu_groups}" ]] \
        && find "${AGENTIC_IOMMU_GROUPS_ROOT:-/sys/kernel/iommu_groups}" -mindepth 1 -maxdepth 1 -print -quit | grep -q .; then
        iommu_enabled=true
    fi
    jq -n \
        --arg checked_at "$(date --utc +%Y-%m-%dT%H:%M:%SZ)" \
        --arg host_class "$(jq -r '.host_class' "$CONFIG")" \
        --arg kernel "$(uname -srmo)" \
        --arg cpu "$(lscpu 2>/dev/null | awk -F: '$1 == "Model name" {sub(/^[[:space:]]+/, "", $2); print $2; exit}')" \
        --arg board_vendor "$(read_trimmed "${AGENTIC_DMI_ROOT:-/sys/class/dmi/id}/board_vendor")" \
        --arg board_name "$(read_trimmed "${AGENTIC_DMI_ROOT:-/sys/class/dmi/id}/board_name")" \
        --arg bios_vendor "$(read_trimmed "${AGENTIC_DMI_ROOT:-/sys/class/dmi/id}/bios_vendor")" \
        --arg bios_version "$(read_trimmed "${AGENTIC_DMI_ROOT:-/sys/class/dmi/id}/bios_version")" \
        --arg service_bdf "$service_bdf" --arg service_driver "$service_driver" \
        --arg service_vendor "$(read_trimmed "$service_dir/vendor")" \
        --arg service_device "$(read_trimmed "$service_dir/device")" \
        --arg test_bdf "$test_bdf" --arg test_driver "$test_driver" \
        --arg test_vendor "$(read_trimmed "$test_dir/vendor")" \
        --arg test_device "$(read_trimmed "$test_dir/device")" \
        --arg group "$group_id" \
        --argjson members "$(printf '%s\n' "${actual_members[@]}" | jq -Rsc 'split("\n") | map(select(length > 0))')" \
        --arg reset_method "$reset_method" \
        --argjson iommu_enabled "$iommu_enabled" \
        '{schema:1,checked_at:$checked_at,host_class:$host_class,kernel:$kernel,cpu:$cpu,
          firmware:{board_vendor:$board_vendor,board_name:$board_name,bios_vendor:$bios_vendor,bios_version:$bios_version},
          iommu_enabled:$iommu_enabled,
          service_gpu:{bdf:$service_bdf,vendor:$service_vendor,device:$service_device,driver:$service_driver},
          test_gpu:{bdf:$test_bdf,vendor:$test_vendor,device:$test_device,driver:$test_driver,iommu_group:$group,group_members:$members,reset_method:(if $reset_method == "" then null else $reset_method end),reset_interface:true},
          secrets_collected:false}'
}

case "$COMMAND" in
    inventory)
        emit_inventory
        ;;
    preflight)
        assert_service_gpu_preserved
        assert_native_test_drivers
        assert_test_gpu_idle
        assert_no_claim
        emit_inventory | jq '.phase="preflight" | .result="pass"'
        ;;
    postflight)
        [[ -n "$VM_NAME" ]] || die "postflight requires --vm"
        assert_service_gpu_preserved
        assert_native_test_drivers
        assert_no_claim
        [[ ! -e "$DEV_ROOT/vfio/$group_id" ]] || die "stale VFIO device node remains for group $group_id"
        assert_no_stale_vmm "$VM_NAME"
        emit_inventory | jq --arg vm "$VM_NAME" '.phase="postflight" | .vm=$vm | .result="pass"'
        ;;
esac
