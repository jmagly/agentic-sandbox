#!/bin/bash
# backends/cloud-hypervisor.sh — Cloud Hypervisor/KVM backend
#
# Implements the same provisioning contract as libvirt for the Phase 0 fast VM
# path. The backend is additive and selected with:
#
#   AGENTIC_BACKEND=cloud-hypervisor
#
# Dependencies: cloud-hypervisor, ch-remote, qemu-img, iproute2, virtiofsd

set -euo pipefail

_ch_install_root="${AGENTIC_CH_INSTALL_ROOT:-/opt/agentic-sandbox/cloud-hypervisor}"

if [[ -n "${AGENTIC_CH_BIN:-}" ]]; then
    _ch_bin="$AGENTIC_CH_BIN"
elif [[ -x "$_ch_install_root/current/bin/cloud-hypervisor" ]]; then
    _ch_bin="$_ch_install_root/current/bin/cloud-hypervisor"
else
    _ch_bin="cloud-hypervisor"
fi

if [[ -n "${AGENTIC_CH_REMOTE_BIN:-}" ]]; then
    _ch_remote_bin="$AGENTIC_CH_REMOTE_BIN"
elif [[ -x "$_ch_install_root/current/bin/ch-remote" ]]; then
    _ch_remote_bin="$_ch_install_root/current/bin/ch-remote"
else
    _ch_remote_bin="ch-remote"
fi

if [[ -n "${AGENTIC_CH_FIRMWARE:-}" ]]; then
    _ch_firmware="$AGENTIC_CH_FIRMWARE"
elif [[ -n "${HYPERVISOR_FW:-}" ]]; then
    _ch_firmware="$HYPERVISOR_FW"
elif [[ -r "$_ch_install_root/current/firmware/CLOUDHV.fd" ]]; then
    _ch_firmware="$_ch_install_root/current/firmware/CLOUDHV.fd"
elif [[ -r "$_ch_install_root/current/firmware/hypervisor-fw" ]]; then
    _ch_firmware="$_ch_install_root/current/firmware/hypervisor-fw"
elif [[ -r "/usr/share/cloud-hypervisor/CLOUDHV.fd" ]]; then
    _ch_firmware="/usr/share/cloud-hypervisor/CLOUDHV.fd"
else
    _ch_firmware="/usr/share/cloud-hypervisor/hypervisor-fw"
fi
_ch_bridge="${AGENTIC_CH_BRIDGE:-${CLOUD_HYPERVISOR_BRIDGE:-virbr0}}"

if [[ -n "${AGENTIC_CH_VIRTIOFSD_BIN:-}" ]]; then
    _ch_virtiofsd_bin="$AGENTIC_CH_VIRTIOFSD_BIN"
elif command -v virtiofsd >/dev/null 2>&1; then
    _ch_virtiofsd_bin="virtiofsd"
elif [[ -x /usr/libexec/virtiofsd ]]; then
    _ch_virtiofsd_bin="/usr/libexec/virtiofsd"
elif [[ -x /usr/lib/qemu/virtiofsd ]]; then
    _ch_virtiofsd_bin="/usr/lib/qemu/virtiofsd"
else
    _ch_virtiofsd_bin="virtiofsd"
fi

_ch_state_dir() {
    local vm_name="$1"
    printf '%s/%s/cloud-hypervisor' "${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}" "$vm_name"
}

_ch_state_file() {
    local vm_name="$1"
    printf '%s/vm.env' "$(_ch_state_dir "$vm_name")"
}

_ch_quote() {
    printf '%q' "$1"
}

_ch_tap_name() {
    local vm_name="$1"
    local hash
    hash="$(printf '%s' "$vm_name" | sha1sum | cut -c1-10)"
    printf 'as%s' "$hash"
}

_ch_bridge_for_network() {
    local network="${1:-default}"
    if [[ -n "${AGENTIC_CH_BRIDGE:-${CLOUD_HYPERVISOR_BRIDGE:-}}" ]]; then
        echo "$_ch_bridge"
    elif [[ "$network" == "default" ]]; then
        echo "virbr0"
    else
        echo "$network"
    fi
}

_ch_require_command() {
    local cmd="$1"
    if ! command -v "$cmd" >/dev/null 2>&1; then
        log_error "Cloud Hypervisor backend requires '$cmd' on PATH"
        return 1
    fi
}

_ch_verify_sha256() {
    local label="$1"
    local path="$2"
    local expected="$3"

    [[ -n "$expected" ]] || return 0
    if [[ ! "$expected" =~ ^[0-9a-fA-F]{64}$ ]]; then
        log_error "Invalid ${label} SHA256 pin: $expected"
        return 1
    fi

    local actual
    actual="$(sha256sum "$path" | awk '{print $1}')"
    if [[ "${actual,,}" != "${expected,,}" ]]; then
        log_error "${label} SHA256 mismatch: expected $expected got $actual ($path)"
        return 1
    fi
}

_ch_truthy() {
    case "${1,,}" in
        1|true|yes|on) return 0 ;;
        *) return 1 ;;
    esac
}

_ch_pci_sysfs_root() {
    printf '%s' "${AGENTIC_CH_PCI_SYSFS_ROOT:-/sys/bus/pci}"
}

_ch_vfio_claim_root() {
    printf '%s' "${AGENTIC_CH_VFIO_CLAIM_ROOT:-${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/.vfio-claims}"
}

_ch_normalize_pci_bdf() {
    local bdf="${1,,}"
    if [[ ! "$bdf" =~ ^[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-7]$ ]]; then
        log_error "Invalid GPU PCI address '$1'; expected domain:bus:slot.function (for example 0000:41:00.0)"
        return 1
    fi
    printf '%s\n' "$bdf"
}

_ch_parse_gpu_config() {
    local config_path="$1"
    local enabled_var="$2"
    local device_var="$3"
    local driver_var="$4"
    local enabled=false device="" driver="vfio-pci"

    if [[ ! -f "$config_path" ]]; then
        log_error "GPU config sidecar not found: $config_path"
        return 1
    fi

    local key value
    while IFS='=' read -r key value; do
        value="${value%$'\r'}"
        case "$key" in
            GPU_ENABLED) enabled="$value" ;;
            GPU_PCI_DEVICE) device="$value" ;;
            GPU_DRIVER) driver="$value" ;;
        esac
    done < "$config_path"

    if ! _ch_truthy "$enabled"; then
        printf -v "$enabled_var" '%s' "false"
        printf -v "$device_var" '%s' ""
        printf -v "$driver_var" '%s' "$driver"
        return 0
    fi
    if [[ "$driver" != "vfio-pci" && "$driver" != "vfio_pci" ]]; then
        log_error "Cloud Hypervisor GPU passthrough requires GPU_DRIVER=vfio-pci, got '$driver'"
        return 1
    fi
    device="$(_ch_normalize_pci_bdf "$device")" || return 1

    printf -v "$enabled_var" '%s' "true"
    printf -v "$device_var" '%s' "$device"
    printf -v "$driver_var" '%s' "vfio-pci"
}

_ch_validate_gpu_pci_class() {
    local bdf="$1"
    local class_file
    class_file="$(_ch_pci_sysfs_root)/devices/$bdf/class"
    if [[ ! -r "$class_file" ]]; then
        log_error "GPU PCI class is unreadable: $class_file"
        return 1
    fi
    local class_code
    class_code="$(tr '[:upper:]' '[:lower:]' < "$class_file")"
    if [[ ! "$class_code" =~ ^0x03[0-9a-f]{4}$ ]]; then
        log_error "Configured GPU primary $bdf has non-display PCI class $class_code"
        return 1
    fi
    return 0
}

_ch_assert_gpu_idle() {
    local bdf="$1"
    local drm_root="${AGENTIC_CH_DRM_CLASS_ROOT:-/sys/class/drm}"
    local dev_root="${AGENTIC_CH_DEV_ROOT:-/dev}"
    local -a device_nodes=()
    local entry name entry_bdf

    for entry in "$drm_root"/card* "$drm_root"/renderD*; do
        [[ -e "$entry/device" ]] || continue
        name="$(basename "$entry")"
        [[ "$name" =~ ^(card|renderD)[0-9]+$ ]] || continue
        entry_bdf="$(basename "$(readlink -f "$entry/device")")"
        if [[ "$entry_bdf" == "$bdf" && -e "$dev_root/dri/$name" ]]; then
            device_nodes+=("$dev_root/dri/$name")
        fi
    done

    if [[ "$(_ch_pci_driver "$bdf")" == "nvidia" ]]; then
        local nvidia_proc_root="${AGENTIC_CH_NVIDIA_PROC_ROOT:-/proc/driver/nvidia/gpus}"
        local nvidia_info="$nvidia_proc_root/$bdf/information"
        local nvidia_index=""
        if [[ -r "$nvidia_info" ]]; then
            nvidia_index="$(awk -F: 'tolower($1) ~ /device minor/ {
                gsub(/[[:space:]]/, "", $2); print $2; exit
            }' "$nvidia_info")"
        else
            local nvidia_smi_bin="${AGENTIC_CH_NVIDIA_SMI_BIN:-nvidia-smi}"
            if ! command -v "$nvidia_smi_bin" >/dev/null 2>&1; then
                log_error "Cannot map NVIDIA GPU $bdf to a device node; nvidia-smi is unavailable"
                return 1
            fi
            local nvidia_inventory nvidia_bdf index bus_slot
            if ! nvidia_inventory="$("$nvidia_smi_bin" \
                --query-gpu=pci.bus_id,index --format=csv,noheader,nounits 2>&1)"; then
                log_error "Cannot map NVIDIA GPU $bdf to a device node; nvidia-smi failed"
                return 1
            fi
            bus_slot="${bdf#*:}"
            while IFS=',' read -r nvidia_bdf index; do
                nvidia_bdf="${nvidia_bdf//[[:space:]]/}"
                index="${index//[[:space:]]/}"
                if [[ "${nvidia_bdf,,}" == *":${bus_slot,,}" ]]; then
                    nvidia_index="$index"
                    break
                fi
            done <<<"$nvidia_inventory"
        fi
        if [[ ! "$nvidia_index" =~ ^[0-9]+$ || ! -e "$dev_root/nvidia$nvidia_index" ]]; then
            log_error "Cannot resolve an NVIDIA device node for GPU $bdf"
            return 1
        fi
        device_nodes+=("$dev_root/nvidia$nvidia_index")
    fi

    local node fuser_output probe_rc
    if (( ${#device_nodes[@]} > 0 )) && ! command -v fuser >/dev/null 2>&1; then
        log_error "fuser is required to prove GPU $bdf is idle before VFIO hand-out"
        return 1
    fi
    for node in "${device_nodes[@]}"; do
        if fuser_output="$(sudo fuser -s "$node" 2>&1)"; then
            log_error "GPU $bdf is active through $node; refusing disruptive VFIO hand-out"
            log_info "Stop the graphical/compute workload using the device before retrying"
            return 1
        else
            probe_rc=$?
            if [[ "$probe_rc" -ne 1 || -n "$fuser_output" ]]; then
                log_error "Could not prove GPU $bdf is idle through $node; refusing VFIO hand-out"
                return 1
            fi
        fi
    done
    return 0
}

_ch_pci_driver() {
    local bdf="$1"
    local driver_link
    driver_link="$(_ch_pci_sysfs_root)/devices/$bdf/driver"
    if [[ -L "$driver_link" ]]; then
        basename "$(readlink -f "$driver_link")"
    fi
}

_ch_sysfs_write() {
    local value="$1"
    local path="$2"
    if [[ -n "${AGENTIC_CH_VFIO_SYSFS_LOG:-}" ]]; then
        printf '%s\t%s\n' "$path" "$value" >> "$AGENTIC_CH_VFIO_SYSFS_LOG"
    fi
    printf '%s\n' "$value" | sudo tee "$path" >/dev/null
}

_ch_vfio_reset_group() {
    local records_file="$1"
    local primary_bdf="$2"
    local phase="$3"
    local pci_root
    pci_root="$(_ch_pci_sysfs_root)"
    local reset_path="$pci_root/devices/$primary_bdf/reset"

    if [[ ! -e "$reset_path" ]]; then
        if _ch_truthy "${AGENTIC_CH_VFIO_ALLOW_NO_RESET:-false}"; then
            if [[ "$phase" == "VFIO teardown" ]] \
                && ! _ch_truthy "${AGENTIC_CH_VFIO_FORCE_RELEASE_AFTER_POWER_CYCLE:-false}"; then
                log_error "GPU $primary_bdf has no PCI reset; keeping its VFIO claim quarantined after use"
                log_info "Power-cycle the host, then set AGENTIC_CH_VFIO_FORCE_RELEASE_AFTER_POWER_CYCLE=1 for managed driver restoration"
                return 1
            fi
            log_warn "GPU $primary_bdf exposes no PCI reset; allowing $phase only because AGENTIC_CH_VFIO_ALLOW_NO_RESET is set"
            return 0
        fi
        log_error "GPU $primary_bdf exposes no PCI reset; refusing $phase to prevent cross-tenant device/VRAM residue"
        log_info "Reserve this GPU for a single tenant or set AGENTIC_CH_VFIO_ALLOW_NO_RESET=1 only after a host-class security review"
        return 1
    fi

    if ! _ch_sysfs_write "1" "$reset_path"; then
        log_error "PCI reset failed for GPU $primary_bdf during $phase"
        return 1
    fi

    # Reset companion functions when supported. The primary reset is mandatory;
    # audio/USB companion functions do not consistently expose their own reset.
    local bdf _original_driver _original_override companion_reset
    while IFS=$'\t' read -r bdf _original_driver _original_override; do
        [[ -n "$bdf" && "$bdf" != "$primary_bdf" ]] || continue
        companion_reset="$pci_root/devices/$bdf/reset"
        if [[ -e "$companion_reset" ]]; then
            if ! _ch_sysfs_write "1" "$companion_reset"; then
                log_error "PCI reset failed for IOMMU companion $bdf during $phase"
                return 1
            fi
        fi
    done < "$records_file"
}

_ch_validate_vfio_state() {
    local vm_name="$1"
    local primary_bdf="$2"
    local records_file="$3"
    local group_file="$4"
    local pci_root
    pci_root="$(_ch_pci_sysfs_root)"
    local primary_dir="$pci_root/devices/$primary_bdf"

    if [[ ! -L "$primary_dir/iommu_group" ]]; then
        log_error "Cannot validate VFIO state: $primary_bdf has no IOMMU group"
        return 1
    fi
    local group_dir group_id recorded_group
    group_dir="$(readlink -f "$primary_dir/iommu_group")"
    group_id="$(basename "$group_dir")"
    if [[ ! -s "$group_file" ]]; then
        log_error "VFIO group metadata is missing or empty: $group_file"
        return 1
    fi
    recorded_group="$(cat "$group_file")"
    if [[ ! "$recorded_group" =~ ^[0-9]+$ || "$recorded_group" != "$group_id" ]]; then
        log_error "VFIO group metadata mismatch: recorded '${recorded_group:-empty}', current '$group_id'"
        return 1
    fi

    local claim_dir owner=""
    claim_dir="$(_ch_vfio_claim_root)/iommu-$group_id"
    [[ -f "$claim_dir/owner" ]] && owner="$(cat "$claim_dir/owner")"
    if [[ "$owner" != "$vm_name" ]]; then
        log_error "VFIO claim owner mismatch for group $group_id: expected '$vm_name', got '${owner:-missing}'"
        return 1
    fi
    if [[ ! -s "$records_file" ]]; then
        log_error "VFIO recovery journal is missing or empty: $records_file"
        return 1
    fi

    local bdf original_driver original_override extra
    local -a journal_devices=()
    while IFS=$'\t' read -r bdf original_driver original_override extra; do
        if [[ -z "$bdf" || -z "$original_driver" || -z "$original_override" || -n "$extra" ]]; then
            log_error "Malformed VFIO recovery journal row in $records_file"
            return 1
        fi
        _ch_normalize_pci_bdf "$bdf" >/dev/null || return 1
        if [[ ! "$original_driver" =~ ^[-A-Za-z0-9_.]+$ \
           || ! "$original_override" =~ ^[-A-Za-z0-9_.]+$ ]]; then
            log_error "Invalid driver metadata for VFIO device $bdf"
            return 1
        fi
        if [[ ! -d "$pci_root/devices/$bdf" ]]; then
            log_error "Journaled VFIO device is no longer present: $bdf"
            return 1
        fi
        journal_devices+=("$bdf")
    done < "$records_file"

    local expected_devices journal_set
    expected_devices="$(find "$group_dir/devices" -mindepth 1 -maxdepth 1 -printf '%f\n' | sort)"
    journal_set="$(printf '%s\n' "${journal_devices[@]}" | sort)"
    if [[ -z "$expected_devices" || "$journal_set" != "$expected_devices" ]]; then
        log_error "VFIO recovery journal does not exactly match IOMMU group $group_id"
        return 1
    fi
}

_ch_release_vfio_group() {
    local vm_name="$1"
    local primary_bdf="$2"
    local records_file="$3"
    local group_file="$4"
    local require_reset="${5:-true}"
    local pci_root
    pci_root="$(_ch_pci_sysfs_root)"
    if ! _ch_validate_vfio_state "$vm_name" "$primary_bdf" "$records_file" "$group_file"; then
        log_error "Refusing VFIO teardown with inconsistent durable state; claim and journal retained"
        return 1
    fi
    local failed=0
    if [[ "$require_reset" == "true" ]]; then
        if ! _ch_vfio_reset_group "$records_file" "$primary_bdf" "VFIO teardown"; then
            log_error "VFIO group remains claimed by $vm_name because teardown reset failed"
            return 1
        fi
    fi

    local bdf original_driver original_override current_driver device_dir already_restored bind_failed
    while IFS=$'\t' read -r bdf original_driver original_override; do
            [[ -n "$bdf" ]] || continue
            device_dir="$pci_root/devices/$bdf"
            current_driver="$(_ch_pci_driver "$bdf")"
            already_restored=false
            if [[ "$current_driver" == "vfio-pci" ]]; then
                _ch_sysfs_write "$bdf" "$pci_root/drivers/vfio-pci/unbind" || failed=1
            elif [[ -n "$current_driver" && "$current_driver" == "$original_driver" ]]; then
                already_restored=true
            elif [[ -n "$current_driver" && "$current_driver" != "$original_driver" ]]; then
                log_error "Refusing to detach unexpected driver '$current_driver' from $bdf during teardown"
                failed=1
                continue
            fi

            bind_failed=false
            if [[ "$already_restored" != "true" && "$original_driver" != "-" && -n "$original_driver" ]]; then
                # A saved override can intentionally differ from the currently
                # bound native driver. Temporarily target the native driver,
                # bind it, then restore the exact saved override.
                if [[ -e "$device_dir/driver_override" ]]; then
                    _ch_sysfs_write "$original_driver" "$device_dir/driver_override" || {
                        failed=1
                        bind_failed=true
                    }
                fi
                if [[ "$bind_failed" != "true" && -e "$pci_root/drivers/$original_driver/bind" ]]; then
                    _ch_sysfs_write "$bdf" "$pci_root/drivers/$original_driver/bind" || {
                        failed=1
                        bind_failed=true
                    }
                else
                    if [[ "$bind_failed" != "true" ]]; then
                        log_error "Original driver '$original_driver' is unavailable; $bdf remains unbound"
                        failed=1
                        bind_failed=true
                    fi
                fi
            fi

            if [[ "$bind_failed" != "true" && -e "$device_dir/driver_override" ]]; then
                if [[ "$original_override" == "-" ]]; then
                    _ch_sysfs_write "" "$device_dir/driver_override" || failed=1
                else
                    _ch_sysfs_write "$original_override" "$device_dir/driver_override" || failed=1
                fi
            fi
    done < "$records_file"

    if [[ "$failed" == "0" ]]; then
        local group_id claim_dir
        group_id="$(cat "$group_file")"
        claim_dir="$(_ch_vfio_claim_root)/iommu-$group_id"
        if ! rm -rf "$claim_dir"; then
            log_error "Failed to remove VFIO claim after safe teardown: $claim_dir"
            return 1
        fi
        if ! rm -f "$records_file" "$group_file"; then
            log_error "VFIO device is safe but stale recovery metadata could not be removed"
            return 1
        fi
    else
        log_error "VFIO group remains claimed by $vm_name; recovery metadata preserved at $records_file"
    fi
    return "$failed"
}

_ch_wait_for_vfio_group_device() {
    local group_id="$1"
    local wait_ms="${2:-${AGENTIC_CH_VFIO_DEVICE_WAIT_MS:-2000}}"
    local vfio_group_device="${AGENTIC_CH_VFIO_DEV_ROOT:-/dev/vfio}/$group_id"
    local attempts=$((wait_ms / 50))
    local _
    (( attempts > 0 )) || attempts=1

    for _ in $(seq 1 "$attempts"); do
        [[ -c "$vfio_group_device" ]] && break
        sleep 0.05
    done
    if [[ ! -c "$vfio_group_device" ]]; then
        log_error "VFIO group character device did not appear after binding: $vfio_group_device"
        return 1
    fi

    # The durable claim grants this backend exclusive ownership. Enforce that
    # boundary even when a host udev rule created a permissive node.
    local expected_owner
    expected_owner="$(id -u):$(id -g)"
    if ! sudo chown "$expected_owner" "$vfio_group_device" \
        || ! sudo chmod 0600 "$vfio_group_device"; then
        log_error "Failed to grant the backend owner-only access to VFIO group device: $vfio_group_device"
        return 1
    fi
    local actual_owner actual_mode
    actual_owner="$(stat -Lc '%u:%g' "$vfio_group_device" 2>/dev/null || true)"
    actual_mode="$(stat -Lc '%a' "$vfio_group_device" 2>/dev/null || true)"
    if [[ ! -c "$vfio_group_device" || ! -r "$vfio_group_device" || ! -w "$vfio_group_device" \
        || "$actual_owner" != "$expected_owner" || "$actual_mode" != "600" ]]; then
        log_error "VFIO group device is not an owner-only readable/writable character device: $vfio_group_device"
        return 1
    fi
    return 0
}

_ch_prepare_vfio_group() {
    local vm_name="$1"
    local primary_bdf="$2"
    local records_file="$3"
    local group_file="$4"
    local pci_root
    pci_root="$(_ch_pci_sysfs_root)"
    local primary_dir="$pci_root/devices/$primary_bdf"

    if [[ ! -d "$primary_dir" ]]; then
        log_error "GPU PCI device not found: $primary_dir"
        return 1
    fi
    if [[ ! -L "$primary_dir/iommu_group" ]]; then
        log_error "GPU $primary_bdf has no IOMMU group; enable host IOMMU before VFIO passthrough"
        return 1
    fi

    local group_dir group_id
    group_dir="$(readlink -f "$primary_dir/iommu_group")"
    group_id="$(basename "$group_dir")"
    if [[ ! -d "$group_dir/devices" ]]; then
        log_error "IOMMU group $group_id has no devices directory: $group_dir/devices"
        return 1
    fi
    local -a group_devices=()
    mapfile -t group_devices < <(find "$group_dir/devices" -mindepth 1 -maxdepth 1 -printf '%f\n' | sort)
    if [[ "${#group_devices[@]}" -eq 0 ]]; then
        log_error "IOMMU group $group_id is empty"
        return 1
    fi

    local primary_slot="${primary_bdf%.*}"
    local bdf
    for bdf in "${group_devices[@]}"; do
        _ch_normalize_pci_bdf "$bdf" >/dev/null || return 1
        if [[ "${bdf%.*}" != "$primary_slot" ]] \
            && ! _ch_truthy "${AGENTIC_CH_VFIO_ALLOW_UNSAFE_GROUP:-false}"; then
            log_error "IOMMU group $group_id contains unrelated device $bdf (GPU is $primary_bdf)"
            log_info "Use an ACS-isolated GPU slot; AGENTIC_CH_VFIO_ALLOW_UNSAFE_GROUP=1 requires a host-class security review"
            return 1
        fi
    done

    local claim_root claim_dir owner=""
    claim_root="$(_ch_vfio_claim_root)"
    claim_dir="$claim_root/iommu-$group_id"
    if ! mkdir -p "$claim_root"; then
        log_error "Cannot create VFIO claim root: $claim_root"
        return 1
    fi

    # A stopped VM retains its complete claim/journal and vfio binding.
    # Validate the durable state before trusting it or launching from it.
    if [[ -d "$claim_dir" ]]; then
        [[ -f "$claim_dir/owner" ]] && owner="$(cat "$claim_dir/owner")"
        if [[ "$owner" != "$vm_name" ]]; then
            log_error "IOMMU group $group_id is already assigned to VM '${owner:-unknown}'"
            return 1
        fi
        if ! _ch_validate_vfio_state "$vm_name" "$primary_bdf" "$records_file" "$group_file"; then
            return 1
        fi
        local _original_driver _original_override
        while IFS=$'\t' read -r bdf _original_driver _original_override; do
            if [[ "$(_ch_pci_driver "$bdf")" != "vfio-pci" ]]; then
                log_error "Claimed VFIO device $bdf is no longer bound to vfio-pci"
                return 1
            fi
        done < "$records_file"
        _ch_vfio_reset_group "$records_file" "$primary_bdf" "VM hand-out"
        return
    fi

    if [[ -e "$records_file" || -e "$group_file" ]]; then
        log_error "Stale VFIO metadata exists without its group claim; refusing to overwrite recovery state"
        return 1
    fi
    if ! mkdir "$claim_dir"; then
        log_error "Failed to claim IOMMU group $group_id"
        return 1
    fi
    if ! printf '%s\n' "$vm_name" > "$claim_dir/owner"; then
        log_error "Failed to persist VFIO claim owner for group $group_id"
        rm -rf "$claim_dir" 2>/dev/null || true
        return 1
    fi
    if ! printf '%s\n' "$group_id" > "$group_file"; then
        log_error "Failed to persist VFIO group metadata: $group_file"
        rm -f "$group_file" 2>/dev/null || true
        rm -rf "$claim_dir" 2>/dev/null || true
        return 1
    fi

    # Persist the complete original-driver journal atomically before the first
    # privileged unbind. Partial journals must never describe mutable hardware.
    local records_tmp
    if ! records_tmp="$(mktemp "${records_file}.tmp.XXXXXX")"; then
        log_error "Failed to create VFIO recovery journal"
        rm -f "$group_file" 2>/dev/null || true
        rm -rf "$claim_dir" 2>/dev/null || true
        return 1
    fi
    local original_driver original_override device_dir
    for bdf in "${group_devices[@]}"; do
        device_dir="$pci_root/devices/$bdf"
        original_driver="$(_ch_pci_driver "$bdf")"
        [[ -n "$original_driver" ]] || original_driver="-"
        original_override="-"
        if [[ -r "$device_dir/driver_override" ]]; then
            original_override="$(cat "$device_dir/driver_override")"
            [[ -n "$original_override" && "$original_override" != "(null)" ]] || original_override="-"
        fi
        if ! printf '%s\t%s\t%s\n' "$bdf" "$original_driver" "$original_override" >> "$records_tmp"; then
            log_error "Failed to persist complete VFIO recovery journal"
            rm -f "$records_tmp" "$group_file" 2>/dev/null || true
            rm -rf "$claim_dir" 2>/dev/null || true
            return 1
        fi
    done
    if ! chmod 600 "$records_tmp" || ! mv "$records_tmp" "$records_file"; then
        log_error "Failed to commit VFIO recovery journal: $records_file"
        rm -f "$records_tmp" "$records_file" "$group_file" 2>/dev/null || true
        rm -rf "$claim_dir" 2>/dev/null || true
        return 1
    fi
    if ! _ch_validate_vfio_state "$vm_name" "$primary_bdf" "$records_file" "$group_file"; then
        rm -f "$records_file" "$group_file" 2>/dev/null || true
        rm -rf "$claim_dir" 2>/dev/null || true
        return 1
    fi
    if ! sudo modprobe vfio-pci; then
        log_error "Failed to load vfio-pci"
        _ch_release_vfio_group "$vm_name" "$primary_bdf" "$records_file" "$group_file" false || true
        return 1
    fi

    local -a vfio_records=()
    mapfile -t vfio_records < "$records_file"
    local record
    for record in "${vfio_records[@]}"; do
        IFS=$'\t' read -r bdf original_driver original_override <<< "$record"
        device_dir="$pci_root/devices/$bdf"
        if [[ "$original_driver" != "vfio-pci" ]]; then
            if [[ ! -e "$device_dir/driver_override" ]]; then
                log_error "PCI device $bdf has no driver_override attribute"
                _ch_release_vfio_group "$vm_name" "$primary_bdf" "$records_file" "$group_file" false || true
                return 1
            fi
            if ! _ch_sysfs_write "vfio-pci" "$device_dir/driver_override"; then
                _ch_release_vfio_group "$vm_name" "$primary_bdf" "$records_file" "$group_file" false || true
                return 1
            fi
            if [[ "$original_driver" != "-" ]]; then
                if ! _ch_sysfs_write "$bdf" "$pci_root/drivers/$original_driver/unbind"; then
                    _ch_release_vfio_group "$vm_name" "$primary_bdf" "$records_file" "$group_file" false || true
                    return 1
                fi
            fi
            if ! _ch_sysfs_write "$bdf" "$pci_root/drivers/vfio-pci/bind"; then
                _ch_release_vfio_group "$vm_name" "$primary_bdf" "$records_file" "$group_file" false || true
                return 1
            fi
        fi
        if [[ "$(_ch_pci_driver "$bdf")" != "vfio-pci" ]]; then
            log_error "Failed to bind IOMMU group member $bdf to vfio-pci"
            _ch_release_vfio_group "$vm_name" "$primary_bdf" "$records_file" "$group_file" false || true
            return 1
        fi
    done

    # The group character device is created asynchronously by VFIO/devtmpfs
    # after at least one group member binds, so wait and provision scoped
    # access only after the complete group has transitioned.
    if [[ "${AGENTIC_CH_SKIP_DEVICE_CHECKS:-0}" != "1" ]]; then
        if ! _ch_wait_for_vfio_group_device "$group_id"; then
            _ch_release_vfio_group "$vm_name" "$primary_bdf" "$records_file" "$group_file" false || true
            return 1
        fi
    fi

    if ! _ch_vfio_reset_group "$records_file" "$primary_bdf" "VM hand-out"; then
        _ch_release_vfio_group "$vm_name" "$primary_bdf" "$records_file" "$group_file" false || true
        return 1
    fi
    log_success "VFIO group $group_id assigned to $vm_name (${group_devices[*]})"
}

_ch_ensure_userfaultfd() {
    local proc_file="${AGENTIC_CH_USERFAULTFD_PROC_FILE:-/proc/sys/vm/unprivileged_userfaultfd}"
    local current
    current="$(cat "$proc_file" 2>/dev/null || echo 0)"
    if [[ "$current" == "1" ]]; then
        return 0
    fi

    if ! _ch_truthy "${AGENTIC_CH_CONFIGURE_USERFAULTFD:-1}"; then
        log_warn "vm.unprivileged_userfaultfd is not enabled; CH snapshot ondemand restore will need CAP_SYS_PTRACE or copy mode"
        log_info "Recommended: sudo sysctl -w vm.unprivileged_userfaultfd=1"
        return 0
    fi

    local drop_in="${AGENTIC_CH_USERFAULTFD_SYSCTL_FILE:-/etc/sysctl.d/99-agentic-cloud-hypervisor.conf}"
    log_info "Configuring vm.unprivileged_userfaultfd=1 for Cloud Hypervisor snapshot restore"
    printf '%s\n' 'vm.unprivileged_userfaultfd=1' | sudo tee "$drop_in" >/dev/null
    sudo sysctl -w vm.unprivileged_userfaultfd=1 >/dev/null
}

_backend_cloud-hypervisor_prepare_host() {
    _ch_require_command "$_ch_bin"
    _ch_require_command "$_ch_remote_bin"
    _ch_require_command "$_ch_virtiofsd_bin"
    _ch_require_command ip

    if [[ ! -r "$_ch_firmware" ]]; then
        log_error "Cloud Hypervisor firmware not found/readable: $_ch_firmware"
        log_info "Set AGENTIC_CH_FIRMWARE to a CLOUDHV.fd or hypervisor-fw path"
        return 1
    fi

    if [[ -n "${AGENTIC_CH_EXPECTED_VERSION:-}" ]]; then
        local version_output
        version_output="$("$_ch_bin" --version 2>&1 || true)"
        if [[ "$version_output" != *"$AGENTIC_CH_EXPECTED_VERSION"* ]]; then
            log_error "Cloud Hypervisor version mismatch: expected substring '$AGENTIC_CH_EXPECTED_VERSION', got '$version_output'"
            return 1
        fi
    fi

    local ch_path
    ch_path="$(command -v "$_ch_bin")"
    _ch_verify_sha256 "Cloud Hypervisor binary" "$ch_path" "${AGENTIC_CH_BIN_SHA256:-}"
    _ch_verify_sha256 "Cloud Hypervisor firmware" "$_ch_firmware" "${AGENTIC_CH_FIRMWARE_SHA256:-}"

    if [[ "${AGENTIC_CH_SKIP_DEVICE_CHECKS:-0}" != "1" ]]; then
        if [[ ! -e /dev/kvm ]]; then
            log_error "Cloud Hypervisor backend requires /dev/kvm"
            return 1
        fi

        if [[ ! -e /dev/vhost-vsock ]]; then
            log_error "Cloud Hypervisor backend requires /dev/vhost-vsock for ADR-023 vsock transport"
            return 1
        fi
    fi

    _ch_ensure_userfaultfd
}

_backend_cloud-hypervisor_grant_storage_access() {
    local vm_dir="$1"
    local cloud_init_dir="$2"
    shift 2

    chmod 700 "$vm_dir" "$cloud_init_dir" 2>/dev/null || sudo chmod 700 "$vm_dir" "$cloud_init_dir"

    local path
    for path in "$@"; do
        if [[ -e "$path" ]]; then
            chmod 600 "$path" 2>/dev/null || sudo chmod 600 "$path"
        fi
    done
}

_backend_cloud-hypervisor_prepare_network() {
    local network="$1"
    local vm_name="$2"
    local mac="$3"
    local ip_addr="$4"
    local bridge
    bridge="$(_ch_bridge_for_network "$network")"
    local tap
    tap="$(_ch_tap_name "$vm_name")"

    if ! ip link show "$bridge" >/dev/null 2>&1; then
        log_error "Cloud Hypervisor bridge not found: $bridge"
        log_info "Set AGENTIC_CH_BRIDGE or pass --network with an existing Linux bridge"
        return 1
    fi

    if ! ip link show "$tap" >/dev/null 2>&1; then
        sudo ip tuntap add dev "$tap" mode tap user "$(id -un)"
    fi
    sudo ip link set dev "$tap" master "$bridge"
    sudo ip link set dev "$tap" up
    log_success "Cloud Hypervisor tap ready: $tap → $bridge ($mac, $ip_addr)"

    # If the bridge is libvirt's default network, keep the DHCP reservation in
    # sync as a compatibility aid. Cloud-init still configures the static IP.
    if command -v virsh >/dev/null 2>&1 && declare -F virsh_cmd >/dev/null 2>&1; then
        virsh_cmd net-update "$network" add ip-dhcp-host \
            "<host mac='$mac' name='$vm_name' ip='$ip_addr'/>" \
            --live --config 2>/dev/null || true
    fi
}

_ch_write_state() {
    local state_file="$1"
    shift
    : > "$state_file"
    local kv key value
    for kv in "$@"; do
        key="${kv%%=*}"
        value="${kv#*=}"
        printf '%s=%s\n' "$key" "$(_ch_quote "$value")" >> "$state_file"
    done
}

_ch_ms_now() {
    # VM lifecycle latency is elapsed time, so use the kernel's monotonic clock
    # instead of wall time, which can jump when a CI runner resynchronizes.
    awk '{printf "%.0f\n", $1 * 1000}' /proc/uptime
}

_ch_json_escape() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    value="${value//$'\n'/\\n}"
    printf '%s' "$value"
}

_ch_copy_disk_for_child() {
    local src="$1"
    local dst="$2"
    mkdir -p "$(dirname "$dst")"
    rm -f "$dst"
    if command -v qemu-img >/dev/null 2>&1; then
        if qemu-img create -f qcow2 -F qcow2 -b "$src" "$dst" >/dev/null 2>&1; then
            return 0
        fi
        rm -f "$dst"
    fi
    if cp --reflink=always "$src" "$dst" 2>/dev/null; then
        return 0
    fi
    cp --sparse=always "$src" "$dst"
}

_ch_link_restore_artifact() {
    local src="$1"
    local dst="$2"
    ln -f "$src" "$dst" 2>/dev/null || cp --reflink=always "$src" "$dst" 2>/dev/null || cp --sparse=always "$src" "$dst"
}

_ch_prepare_restore_source() {
    local snapshot_dir="$1"
    local restore_source_dir="$2"
    local child_disk="$3"
    local cloud_init_iso="$4"
    local tap="$5"
    local mac="$6"
    local vsock_cid="$7"
    local vsock_socket="$8"
    local serial_log="$9"
    local child_state_dir="${10}"
    local source_url="file://$snapshot_dir/ch-state"
    if [[ -f "$snapshot_dir/backend-metadata.json" ]]; then
        source_url="$(sed -n 's/.*"snapshot_url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$snapshot_dir/backend-metadata.json" | head -n1)"
        [[ -n "$source_url" ]] || source_url="file://$snapshot_dir/ch-state"
    fi
    if [[ "$source_url" != file://* ]]; then
        log_error "Cloud Hypervisor restore source patching requires a local file:// snapshot: $source_url"
        return 1
    fi
    local ch_state="${source_url#file://}"
    for required in config.json state.json memory-ranges; do
        if [[ ! -f "$ch_state/$required" ]]; then
            log_error "Cloud Hypervisor snapshot artifact missing: $ch_state/$required"
            return 1
        fi
    done
    if ! command -v jq >/dev/null 2>&1; then
        log_error "Cloud Hypervisor restore requires jq to patch per-child snapshot config"
        return 1
    fi

    rm -rf "$restore_source_dir"
    mkdir -p "$restore_source_dir"
    _ch_link_restore_artifact "$ch_state/state.json" "$restore_source_dir/state.json"
    _ch_link_restore_artifact "$ch_state/memory-ranges" "$restore_source_dir/memory-ranges"
    jq \
        --arg disk "$child_disk" \
        --arg cloud_init "$cloud_init_iso" \
        --arg tap "$tap" \
        --arg mac "$mac" \
        --argjson cid "$vsock_cid" \
        --arg vsock "$vsock_socket" \
        --arg serial "$serial_log" \
        --arg child_state "$child_state_dir" \
        '
        .disks[0].path = $disk
        | .disks[0].backing_files = true
        | if ($cloud_init != "" and (.disks | length) > 1) then .disks[1].path = $cloud_init else . end
        | if ((.net // []) | length) > 0 then
            .net[0].tap = $tap
            | .net[0].mac = $mac
            | del(.net[0].host_mac)
          else . end
        | .vsock.cid = $cid
        | .vsock.socket = $vsock
        | .serial.file = $serial
        | if .fs then .fs |= map(.socket = ($child_state + "/" + .tag + ".sock")) else . end
        ' "$ch_state/config.json" > "$restore_source_dir/config.json"
    printf 'file://%s\n' "$restore_source_dir"
}

_backend_cloud-hypervisor_create_vm() {
    local vm_name="$1"
    local disk_path="$2"
    local cloud_init_iso="$3"
    local cpus="$4"
    local memory_mb="$5"
    local network="$6"
    local mac_address="${7:-}"
    local use_agentshare="${8:-false}"
    local inbox_path="${9:-}"
    local outbox_path="${10:-}"
    local mem_limit_mb="${11:-$memory_mb}"
    local cpu_quota_pct="${12:-$((cpus * 100))}"
    local io_weight="${13:-500}"
    local io_read_bps="${14:-524288000}"
    local io_write_bps="${15:-209715200}"
    local gpu_config_path="${16:-}"
    local carbonyl_session_path="${17:-}"
    local vsock_cid="${18:-}"

    if [[ -z "$vsock_cid" ]]; then
        log_error "Cloud Hypervisor backend requires a per-VM vsock CID"
        return 1
    fi

    local gpu_enabled=false gpu_pci_device="" gpu_driver="vfio-pci"
    if [[ -n "$gpu_config_path" ]]; then
        _ch_parse_gpu_config "$gpu_config_path" gpu_enabled gpu_pci_device gpu_driver || return 1
    fi

    local state_dir
    state_dir="$(_ch_state_dir "$vm_name")"
    mkdir -p "$state_dir"

    local api_socket="$state_dir/api.sock"
    local pid_file="$state_dir/pid"
    local serial_log="$state_dir/serial.log"
    local vmm_log="$state_dir/vmm.log"
    local tap
    tap="$(_ch_tap_name "$vm_name")"
    local bridge
    bridge="$(_ch_bridge_for_network "$network")"
    local vsock_socket="$state_dir/vsock.sock"
    local vfio_devices_file="$state_dir/vfio-devices.tsv"
    local vfio_group_file="$state_dir/vfio-group"

    _ch_write_state "$(_ch_state_file "$vm_name")" \
        "VM_NAME=$vm_name" \
        "DISK_PATH=$disk_path" \
        "CLOUD_INIT_ISO=$cloud_init_iso" \
        "CPUS=$cpus" \
        "MEMORY_MB=$memory_mb" \
        "NETWORK=$network" \
        "BRIDGE=$bridge" \
        "MAC_ADDRESS=$mac_address" \
        "TAP_NAME=$tap" \
        "USE_AGENTSHARE=$use_agentshare" \
        "INBOX_PATH=$inbox_path" \
        "OUTBOX_PATH=$outbox_path" \
        "CARBONYL_SESSION_PATH=$carbonyl_session_path" \
        "VSOCK_CID=$vsock_cid" \
        "VSOCK_SOCKET=$vsock_socket" \
        "API_SOCKET=$api_socket" \
        "PID_FILE=$pid_file" \
        "SERIAL_LOG=$serial_log" \
        "VMM_LOG=$vmm_log" \
        "MEM_LIMIT_MB=$mem_limit_mb" \
        "CPU_QUOTA_PCT=$cpu_quota_pct" \
        "IO_WEIGHT=$io_weight" \
        "IO_READ_BPS=$io_read_bps" \
        "IO_WRITE_BPS=$io_write_bps" \
        "GPU_ENABLED=$gpu_enabled" \
        "GPU_PCI_DEVICE=$gpu_pci_device" \
        "GPU_DRIVER=$gpu_driver" \
        "VFIO_DEVICES_FILE=$vfio_devices_file" \
        "VFIO_GROUP_FILE=$vfio_group_file"

    _ch_state_file "$vm_name"
}

_ch_start_virtiofsd() {
    local state_dir="$1"
    local tag="$2"
    local shared_dir="$3"
    local socket="$state_dir/${tag}.sock"
    local pid_file="$state_dir/${tag}.virtiofsd.pid"
    local log_file="$state_dir/${tag}.virtiofsd.log"

    [[ -n "$shared_dir" ]] || return 0
    [[ -d "$shared_dir" ]] || return 0

    rm -f "$socket"
    nohup "$_ch_virtiofsd_bin" \
        --socket-path="$socket" \
        --shared-dir="$shared_dir" \
        --sandbox=none \
        --cache=auto >"$log_file" 2>&1 &
    echo "$!" > "$pid_file"

    local _
    for _ in $(seq 1 50); do
        [[ -S "$socket" ]] && break
        if ! kill -0 "$(cat "$pid_file")" 2>/dev/null; then
            log_error "virtiofsd exited before creating socket for $tag: $socket"
            sed -n '1,20p' "$log_file" >&2 2>/dev/null || true
            return 1
        fi
        sleep 0.1
    done
    if [[ ! -S "$socket" ]]; then
        log_error "virtiofsd did not create socket for $tag: $socket"
        sed -n '1,20p' "$log_file" >&2 2>/dev/null || true
        return 1
    fi
    printf '%s\n' "tag=$tag,socket=$socket"
}

_ch_build_fs_args() {
    local state_dir="$1"
    local use_agentshare="$2"
    local inbox_path="$3"
    local outbox_path="$4"
    local carbonyl_session_path="$5"
    local out_name="$6"
    local fs_spec
    eval "$out_name=()"
    if [[ "$use_agentshare" == "true" ]]; then
        fs_spec="$(_ch_start_virtiofsd "$state_dir" "agentglobal" "${AGENTSHARE_ROOT:-/srv/agentshare}/global-ro")"
        [[ -n "$fs_spec" ]] && eval "$out_name+=(--fs \"\$fs_spec\")"
        fs_spec="$(_ch_start_virtiofsd "$state_dir" "agentinbox" "$inbox_path")"
        [[ -n "$fs_spec" ]] && eval "$out_name+=(--fs \"\$fs_spec\")"
        fs_spec="$(_ch_start_virtiofsd "$state_dir" "agentoutbox" "$outbox_path")"
        [[ -n "$fs_spec" ]] && eval "$out_name+=(--fs \"\$fs_spec\")"
    fi
    fs_spec="$(_ch_start_virtiofsd "$state_dir" "carbonylsessions" "${carbonyl_session_path:-}" || true)"
    [[ -n "$fs_spec" ]] && eval "$out_name+=(--fs \"\$fs_spec\")"
    return 0
}

_ch_proc_mem_kb() {
    local pid="$1" field="$2"
    [[ -n "$pid" && -r "/proc/$pid/smaps_rollup" ]] || {
        if [[ "$field" == "Rss" && -r "/proc/$pid/status" ]]; then
            awk '/^VmRSS:/ { print $2; found=1; exit } END { if (!found) print 0 }' "/proc/$pid/status"
        else
            printf '0\n'
        fi
        return 0
    }
    awk -v field="${field}:" '$1 == field { print $2; found=1; exit } END { if (!found) print 0 }' "/proc/$pid/smaps_rollup"
}

_ch_guest_ram_maps_json() {
    local pid="$1"
    local smaps_path="${2:-/proc/$pid/smaps}"
    if [[ -z "$pid" || ! -r "$smaps_path" ]] || ! command -v jq >/dev/null 2>&1; then
        printf '{"available":false,"reason":"guest RAM smaps unavailable","mappings":[]}'
        return 0
    fi

    local snapshot_file_mapped=false
    if grep -q '/memory-ranges\([[:space:]]\|$\)' "$smaps_path"; then
        snapshot_file_mapped=true
    fi

    awk '
        function emit() {
            if (path ~ /^\/memfd:ch_ram([[:space:]]|$)/) {
                printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", \
                    inode, path, size, rss, pss, shared_clean, shared_dirty, \
                    private_clean, private_dirty, anonymous, ksm, swap
            }
        }
        /^[0-9a-f]+-[0-9a-f]+[[:space:]]/ {
            emit()
            inode=$5
            path=""
            for (i=6; i<=NF; i++) path = path (i == 6 ? "" : " ") $i
            size=rss=pss=shared_clean=shared_dirty=private_clean=private_dirty=anonymous=ksm=swap=0
            next
        }
        $1 == "Size:"          { size=$2 }
        $1 == "Rss:"           { rss=$2 }
        $1 == "Pss:"           { pss=$2 }
        $1 == "Shared_Clean:"  { shared_clean=$2 }
        $1 == "Shared_Dirty:"  { shared_dirty=$2 }
        $1 == "Private_Clean:" { private_clean=$2 }
        $1 == "Private_Dirty:" { private_dirty=$2 }
        $1 == "Anonymous:"     { anonymous=$2 }
        $1 == "KSM:"           { ksm=$2 }
        $1 == "Swap:"          { swap=$2 }
        END { emit() }
    ' "$smaps_path" | jq -Rn --argjson snapshot_file_mapped "$snapshot_file_mapped" '
        [inputs | split("\t") | {
            inode: (.[0] | tonumber),
            path: .[1],
            size_kb: (.[2] | tonumber),
            rss_kb: (.[3] | tonumber),
            pss_kb: (.[4] | tonumber),
            shared_clean_kb: (.[5] | tonumber),
            shared_dirty_kb: (.[6] | tonumber),
            private_clean_kb: (.[7] | tonumber),
            private_dirty_kb: (.[8] | tonumber),
            anonymous_kb: (.[9] | tonumber),
            ksm_kb: (.[10] | tonumber),
            swap_kb: (.[11] | tonumber)
        }] as $mappings |
        {
            available: ($mappings | length > 0),
            measurement: "/proc/<vmm-pid>/smaps entries named /memfd:ch_ram",
            snapshot_memory_ranges_mapped: $snapshot_file_mapped,
            mapping_count: ($mappings | length),
            inodes: ($mappings | map(.inode)),
            virtual_kb: ($mappings | map(.size_kb) | add // 0),
            rss_kb: ($mappings | map(.rss_kb) | add // 0),
            pss_kb: ($mappings | map(.pss_kb) | add // 0),
            shared_clean_kb: ($mappings | map(.shared_clean_kb) | add // 0),
            shared_dirty_kb: ($mappings | map(.shared_dirty_kb) | add // 0),
            private_clean_kb: ($mappings | map(.private_clean_kb) | add // 0),
            private_dirty_kb: ($mappings | map(.private_dirty_kb) | add // 0),
            ksm_kb: ($mappings | map(.ksm_kb) | add // 0),
            swap_kb: ($mappings | map(.swap_kb) | add // 0),
            pss_sharing_savings_kb: (
                (($mappings | map(.rss_kb) | add // 0) - ($mappings | map(.pss_kb) | add // 0)) |
                if . < 0 then 0 else . end
            ),
            mappings: $mappings
        }
    '
}

_ch_snapshot_backing_json() {
    local path="$1"
    if [[ ! -f "$path" ]] || ! command -v jq >/dev/null 2>&1; then
        printf '{"available":false}'
        return 0
    fi
    jq -n \
        --arg path "$path" \
        --arg device_id "$(stat -Lc '%d' "$path")" \
        --arg device_hex "$(stat -Lc '%D' "$path")" \
        --argjson inode "$(stat -Lc '%i' "$path")" \
        --argjson hardlink_count "$(stat -Lc '%h' "$path")" \
        --argjson size_bytes "$(stat -Lc '%s' "$path")" \
        --arg mode "$(stat -Lc '%a' "$path")" \
        '{available:true,path:$path,device_id:$device_id,device_hex:$device_hex,inode:$inode,hardlink_count:$hardlink_count,size_bytes:$size_bytes,mode:$mode}'
}

_ch_prepare_child_agentshare() {
    local child_name="$1"
    local use_agentshare="${2:-false}"
    local source_inbox="${3:-}"
    local out_inbox_var="$4"
    local out_outbox_var="$5"
    local child_inbox="$source_inbox"
    local child_outbox="${OUTBOX_PATH:-}"

    if [[ "$use_agentshare" == "true" && -n "$source_inbox" ]]; then
        child_inbox="${AGENTSHARE_ROOT:-/srv/agentshare}/${child_name}-inbox"
        child_outbox="${AGENTSHARE_ROOT:-/srv/agentshare}/${child_name}-outbox"
        # Production agentshare roots are normally root-owned. Keep the VMM and
        # virtiofsd unprivileged by using sudo only to create/chown each child's
        # top-level directories, then populate their fixed subdirectories as
        # the invoking user. Test and user-owned roots never take this branch.
        if ! mkdir -p "$child_inbox" "$child_outbox" 2>/dev/null; then
            sudo -n install -d -m 0700 -o "$(id -u)" -g "$(id -g)" \
                "$child_inbox" "$child_outbox"
        fi
        mkdir -p "$child_inbox"/{outputs,logs,runs}
        mkdir -p "$child_outbox"/{progress,artifacts}
        chmod 700 "$child_inbox" "$child_outbox" 2>/dev/null || true
        chmod 700 "$child_inbox"/{outputs,logs,runs} "$child_outbox"/{progress,artifacts} 2>/dev/null || true
    fi

    printf -v "$out_inbox_var" '%s' "$child_inbox"
    printf -v "$out_outbox_var" '%s' "$child_outbox"
}

# shellcheck disable=SC2153 # VM metadata variables are loaded from vm.env.
_backend_cloud-hypervisor_start_vm() {
    local vm_name="$1"
    local state_file
    state_file="$(_ch_state_file "$vm_name")"
    if [[ ! -f "$state_file" ]]; then
        log_error "Cloud Hypervisor VM metadata missing: $state_file"
        return 1
    fi

    # shellcheck source=/dev/null
    source "$state_file"

    if [[ -f "$PID_FILE" ]] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
        log_info "Cloud Hypervisor VM already running: $vm_name"
        return 0
    fi

    if ! _backend_cloud-hypervisor_prepare_network "$NETWORK" "$VM_NAME" "$MAC_ADDRESS" ""; then
        return 1
    fi

    local state_dir
    state_dir="$(_ch_state_dir "$VM_NAME")"
    local vmm_log="${VMM_LOG:-$state_dir/vmm.log}"
    rm -f "$API_SOCKET" "$VSOCK_SOCKET"
    : > "$SERIAL_LOG"
    : > "$vmm_log"

    local -a fs_args=()
    _ch_build_fs_args "$state_dir" "$USE_AGENTSHARE" "$INBOX_PATH" "$OUTBOX_PATH" "${CARBONYL_SESSION_PATH:-}" fs_args

    local -a device_args=()
    if _ch_truthy "${GPU_ENABLED:-false}"; then
        _ch_validate_gpu_pci_class "$GPU_PCI_DEVICE" || return 1
        _ch_assert_gpu_idle "$GPU_PCI_DEVICE" || return 1
        if ! _ch_prepare_vfio_group "$VM_NAME" "$GPU_PCI_DEVICE" "$VFIO_DEVICES_FILE" "$VFIO_GROUP_FILE"; then
            return 1
        fi
        local vfio_bdf _original_driver _original_override
        while IFS=$'\t' read -r vfio_bdf _original_driver _original_override; do
            [[ -n "$vfio_bdf" ]] || continue
            device_args+=(--device "path=$(_ch_pci_sysfs_root)/devices/$vfio_bdf/")
        done < "$VFIO_DEVICES_FILE"
    fi

    local -a cmd=(
        "$_ch_bin"
        --api-socket "$API_SOCKET"
        --firmware "$_ch_firmware"
        --disk "path=$DISK_PATH,image_type=qcow2,pci_device_id=1"
        --disk "path=$CLOUD_INIT_ISO,readonly=on,pci_device_id=2"
        --cpus "boot=$CPUS"
        --memory "size=${MEMORY_MB}M,shared=on"
        --net "tap=$TAP_NAME,mac=$MAC_ADDRESS"
        --vsock "cid=$VSOCK_CID,socket=$VSOCK_SOCKET"
        --serial "file=$SERIAL_LOG"
        --console off
    )
    cmd+=("${fs_args[@]}")
    cmd+=("${device_args[@]}")

    nohup "${cmd[@]}" >"$vmm_log" 2>&1 &
    echo "$!" > "$PID_FILE"
    if ! _ch_wait_for_api_ready "$API_SOCKET" "$PID_FILE" "$vmm_log" "${AGENTIC_CH_START_API_WAIT_MS:-2000}"; then
        if ! _backend_cloud-hypervisor_stop_vm "$VM_NAME"; then
            log_error "Cloud Hypervisor is still running; retaining VFIO binding and recovery state for $VM_NAME"
            return 1
        fi
        if _ch_truthy "${GPU_ENABLED:-false}"; then
            _ch_release_vfio_group "$VM_NAME" "$GPU_PCI_DEVICE" "$VFIO_DEVICES_FILE" "$VFIO_GROUP_FILE" || true
        fi
        return 1
    fi
}

_ch_wait_for_api_ready() {
    local api_socket="$1"
    local pid_file="$2"
    local log_file="$3"
    local wait_ms="${4:-1000}"
    local attempts=$((wait_ms / 50))
    local _
    (( attempts > 0 )) || attempts=1
    for _ in $(seq 1 "$attempts"); do
        if [[ -S "$api_socket" ]] && "$_ch_remote_bin" --api-socket "$api_socket" info >/dev/null 2>&1; then
            return 0
        fi
        if [[ -f "$pid_file" ]] && ! kill -0 "$(cat "$pid_file")" 2>/dev/null; then
            log_error "Cloud Hypervisor exited before API became ready: $api_socket"
            sed -n '1,40p' "$log_file" >&2 2>/dev/null || true
            return 1
        fi
        sleep 0.05
    done
    log_error "Cloud Hypervisor API did not become ready within ${wait_ms}ms: $api_socket"
    sed -n '1,40p' "$log_file" >&2 2>/dev/null || true
    return 1
}

_backend_cloud-hypervisor_snapshot_vm() {
    local vm_name="$1"
    local snapshot_dir="$2"
    local snapshot_url="${3:-file://$snapshot_dir/ch-state}"
    local state_file
    state_file="$(_ch_state_file "$vm_name")"
    if [[ ! -f "$state_file" ]]; then
        log_error "Cloud Hypervisor VM metadata missing: $state_file"
        return 1
    fi
    # shellcheck source=/dev/null
    source "$state_file"
    if _ch_truthy "${GPU_ENABLED:-false}"; then
        log_error "Cloud Hypervisor snapshots are disabled for GPU/VFIO VMs: generic vfio-pci devices do not provide migratable device state"
        log_info "Use a cold GPU VM hand-out; destroy it to reset the device before assigning the next tenant"
        return 1
    fi
    if [[ ! -S "$API_SOCKET" ]]; then
        log_error "Cloud Hypervisor API socket missing for snapshot: $API_SOCKET"
        return 1
    fi

    mkdir -p "$snapshot_dir"
    if [[ "$snapshot_url" == file://* ]]; then
        mkdir -p "${snapshot_url#file://}"
    fi
    local start_ms end_ms
    start_ms="$(_ch_ms_now)"
    local current_state
    current_state="$("$_ch_remote_bin" --api-socket "$API_SOCKET" info 2>/dev/null \
        | jq -r '.state // empty' 2>/dev/null || true)"
    if [[ "$current_state" != "Paused" ]]; then
        "$_ch_remote_bin" --api-socket "$API_SOCKET" pause
    fi
    "$_ch_remote_bin" --api-socket "$API_SOCKET" snapshot "$snapshot_url"
    end_ms="$(_ch_ms_now)"

    cp "$state_file" "$snapshot_dir/source-vm.env"
    cat > "$snapshot_dir/backend-metadata.json" <<EOF
{
  "backend": "cloud-hypervisor",
  "source_vm": "$(_ch_json_escape "$vm_name")",
  "source_disk": "$(_ch_json_escape "$DISK_PATH")",
  "snapshot_url": "$(_ch_json_escape "$snapshot_url")",
  "snapshot_dir": "$(_ch_json_escape "$snapshot_dir")",
  "duration_ms": $((end_ms - start_ms))
}
EOF
}

_backend_cloud-hypervisor_restore_vm() {
    local child_name="$1"
    local snapshot_dir="$2"
    local child_disk="$3"
    local vsock_cid="$4"
    local memory_restore_mode="${5:-ondemand}"
    local resume="${6:-true}"

    local source_state="$snapshot_dir/source-vm.env"
    if [[ ! -f "$source_state" ]]; then
        log_error "Cloud Hypervisor snapshot missing source metadata: $source_state"
        return 1
    fi
    # shellcheck source=/dev/null
    source "$source_state"
    if _ch_truthy "${GPU_ENABLED:-false}"; then
        log_error "Refusing to restore a GPU/VFIO snapshot; warm-pool and fork hand-outs cannot safely share generic vfio-pci device state"
        return 1
    fi

    if [[ -z "$vsock_cid" ]]; then
        log_error "Cloud Hypervisor restore requires a fresh per-child vsock CID"
        return 1
    fi
    if [[ ! -f "$child_disk" ]]; then
        _ch_copy_disk_for_child "$DISK_PATH" "$child_disk"
    fi

    local child_state_dir
    child_state_dir="$(_ch_state_dir "$child_name")"
    mkdir -p "$child_state_dir"
    local api_socket="$child_state_dir/api.sock"
    local pid_file="$child_state_dir/pid"
    local serial_log="$child_state_dir/serial.log"
    local vmm_log="$child_state_dir/vmm.log"
    local restore_source_dir="$child_state_dir/restore-source"
    local vsock_socket="$child_state_dir/vsock.sock"
    local tap
    tap="$(_ch_tap_name "$child_name")"
    local mac
    if declare -F generate_mac_from_name >/dev/null 2>&1; then
        mac="$(generate_mac_from_name "$child_name")"
    else
        mac="$MAC_ADDRESS"
    fi

    # Restored guests use a generic DHCP profile baked by clean-prepare. Keep
    # the host-side allocation authoritative, and serialize it because fork
    # restores are intentionally launched in parallel.
    local ip_addr=""
    if declare -F allocate_ip_for_vm >/dev/null 2>&1; then
        mkdir -p "$(dirname "$IP_REGISTRY")"
        local ip_lock_fd
        exec {ip_lock_fd}>"${IP_REGISTRY}.lock"
        flock "$ip_lock_fd"
        ip_addr="$(allocate_ip_for_vm "$child_name" "$NETWORK")"
        flock -u "$ip_lock_fd"
    fi

    local child_inbox_path child_outbox_path
    _ch_prepare_child_agentshare "$child_name" "${USE_AGENTSHARE:-false}" "${INBOX_PATH:-}" child_inbox_path child_outbox_path

    _ch_write_state "$(_ch_state_file "$child_name")" \
        "VM_NAME=$child_name" \
        "DISK_PATH=$child_disk" \
        "CLOUD_INIT_ISO=${CLOUD_INIT_ISO:-}" \
        "CPUS=$CPUS" \
        "MEMORY_MB=$MEMORY_MB" \
        "NETWORK=$NETWORK" \
        "BRIDGE=${BRIDGE:-$(_ch_bridge_for_network "$NETWORK")}" \
        "MAC_ADDRESS=$mac" \
        "IP_ADDRESS=$ip_addr" \
        "TAP_NAME=$tap" \
        "USE_AGENTSHARE=${USE_AGENTSHARE:-false}" \
        "INBOX_PATH=$child_inbox_path" \
        "OUTBOX_PATH=$child_outbox_path" \
        "CARBONYL_SESSION_PATH=${CARBONYL_SESSION_PATH:-}" \
        "VSOCK_CID=$vsock_cid" \
        "VSOCK_SOCKET=$vsock_socket" \
        "API_SOCKET=$api_socket" \
        "PID_FILE=$pid_file" \
        "SERIAL_LOG=$serial_log" \
        "VMM_LOG=$vmm_log" \
        "MEM_LIMIT_MB=${MEM_LIMIT_MB:-$MEMORY_MB}" \
        "CPU_QUOTA_PCT=${CPU_QUOTA_PCT:-$((CPUS * 100))}" \
        "IO_WEIGHT=${IO_WEIGHT:-500}" \
        "IO_READ_BPS=${IO_READ_BPS:-524288000}" \
        "IO_WRITE_BPS=${IO_WRITE_BPS:-209715200}"

    # ch-faststart defines this hook when restore-time enrollment material must
    # be staged. Run it before the VMM resumes so the guest can observe its
    # one-time credential on the first post-restore agent execution.
    if declare -F ch_restore_prelaunch_hook >/dev/null 2>&1; then
        ch_restore_prelaunch_hook "$child_name" "$child_inbox_path" || return 1
    fi

    _backend_cloud-hypervisor_prepare_network "$NETWORK" "$child_name" "$mac" "$ip_addr" >&2
    rm -f "$api_socket" "$vsock_socket"
    : > "$serial_log"
    : > "$vmm_log"

    local -a fs_args=()
    _ch_build_fs_args "$child_state_dir" "${USE_AGENTSHARE:-false}" "$child_inbox_path" "$child_outbox_path" "${CARBONYL_SESSION_PATH:-}" fs_args

    local source_url
    source_url="$(_ch_prepare_restore_source "$snapshot_dir" "$restore_source_dir" "$child_disk" "${CLOUD_INIT_ISO:-}" "$tap" "$mac" "$vsock_cid" "$vsock_socket" "$serial_log" "$child_state_dir")"
    if jq -e '(.fs // []) | length > 0' "$restore_source_dir/config.json" >/dev/null; then
        log_error "Cloud Hypervisor clean-base snapshot contains virtiofs devices; detach vhost-user shares before capture so restore can hot-add fresh child shares"
        return 1
    fi
    local restored_net_count
    restored_net_count="$(jq -r '(.net // []) | length' "$restore_source_dir/config.json")"
    if (( restored_net_count > 1 )); then
        log_error "Cloud Hypervisor clean-base snapshot contains more than one network device"
        return 1
    fi

    local start_ms end_ms
    start_ms="$(_ch_ms_now)"
    local -a cmd=(
        "$_ch_bin"
        --api-socket "$api_socket"
        # Materialize the restored VM in a paused state. Child-specific NIC
        # and virtiofs devices must exist before its first resumed instruction
        # so network/credential activation is deterministic.
        --restore "source_url=$source_url,memory_restore_mode=$memory_restore_mode,resume=false"
    )
    # Restore must be a restore-only CLI invocation. Supplying firmware or
    # device flags makes v53 boot a new VM instead; the child-specific
    # virtiofs sockets are patched into the snapshot config above.
    nohup "${cmd[@]}" >"$vmm_log" 2>&1 &
    echo "$!" > "$pid_file"
    _ch_wait_for_api_ready "$api_socket" "$pid_file" "$vmm_log" "${AGENTIC_CH_RESTORE_API_WAIT_MS:-1000}" || return 1
    if (( restored_net_count == 0 )); then
        "$_ch_remote_bin" --api-socket "$api_socket" add-net \
            "tap=$tap,mac=$mac,id=restore-net" >/dev/null
    fi
    if [[ "${USE_AGENTSHARE:-false}" == "true" ]]; then
        # Add the credential-bearing inbox first. Its virtio hotplug event
        # starts the guest bootstrap one-shot; adding another share first can
        # make that service run before the inbox exists and coalesce the later
        # events while it is still active.
        "$_ch_remote_bin" --api-socket "$api_socket" add-fs \
            "tag=agentinbox,socket=$child_state_dir/agentinbox.sock,id=restore-agentinbox" >/dev/null
        "$_ch_remote_bin" --api-socket "$api_socket" add-fs \
            "tag=agentglobal,socket=$child_state_dir/agentglobal.sock,id=restore-agentglobal" >/dev/null
        "$_ch_remote_bin" --api-socket "$api_socket" add-fs \
            "tag=agentoutbox,socket=$child_state_dir/agentoutbox.sock,id=restore-agentoutbox" >/dev/null
    fi
    if _ch_truthy "$resume"; then
        "$_ch_remote_bin" --api-socket "$api_socket" resume >/dev/null
    fi
    end_ms="$(_ch_ms_now)"
    local vmm_pid rss_kb pss_kb shared_clean_kb shared_dirty_kb guest_ram_json snapshot_backing_json
    vmm_pid="$(cat "$pid_file" 2>/dev/null || true)"
    rss_kb="$(_ch_proc_mem_kb "$vmm_pid" Rss)"
    pss_kb="$(_ch_proc_mem_kb "$vmm_pid" Pss)"
    shared_clean_kb="$(_ch_proc_mem_kb "$vmm_pid" Shared_Clean)"
    shared_dirty_kb="$(_ch_proc_mem_kb "$vmm_pid" Shared_Dirty)"
    guest_ram_json="$(_ch_guest_ram_maps_json "$vmm_pid")"
    snapshot_backing_json="$(_ch_snapshot_backing_json "$restore_source_dir/memory-ranges")"
    cat > "$child_state_dir/restore-metrics.json" <<EOF
{
  "backend": "cloud-hypervisor",
  "source_snapshot": "$(_ch_json_escape "$snapshot_dir")",
  "child_vm": "$(_ch_json_escape "$child_name")",
  "memory_restore_mode": "$(_ch_json_escape "$memory_restore_mode")",
  "vmm_pid": ${vmm_pid:-0},
  "vmm_rss_kb": ${rss_kb:-0},
  "vmm_pss_kb": ${pss_kb:-0},
  "vmm_shared_clean_kb": ${shared_clean_kb:-0},
  "vmm_shared_dirty_kb": ${shared_dirty_kb:-0},
  "guest_ram": $guest_ram_json,
  "snapshot_backing": $snapshot_backing_json,
  "duration_ms": $((end_ms - start_ms))
}
EOF
    printf '%s\n' "$child_state_dir/restore-metrics.json"
}

_backend_cloud-hypervisor_sample_vm_memory() {
    local child_name="$1"
    local child_state_dir
    child_state_dir="$(_ch_state_dir "$child_name")"
    local metrics_path="$child_state_dir/restore-metrics.json"
    local pid_file="$child_state_dir/pid"
    [[ -f "$metrics_path" && -f "$pid_file" ]] || {
        log_error "restore metrics or VMM pid missing for $child_name"
        return 1
    }
    local pid rss pss shared_clean shared_dirty guest_ram snapshot_backing metrics_tmp
    pid="$(cat "$pid_file" 2>/dev/null || true)"
    [[ -n "$pid" && -r "/proc/$pid/smaps" ]] || {
        log_error "running VMM smaps unavailable for $child_name"
        return 1
    }
    rss="$(_ch_proc_mem_kb "$pid" Rss)"
    pss="$(_ch_proc_mem_kb "$pid" Pss)"
    shared_clean="$(_ch_proc_mem_kb "$pid" Shared_Clean)"
    shared_dirty="$(_ch_proc_mem_kb "$pid" Shared_Dirty)"
    guest_ram="$(_ch_guest_ram_maps_json "$pid")"
    snapshot_backing="$(_ch_snapshot_backing_json "$child_state_dir/restore-source/memory-ranges")"
    metrics_tmp="$(mktemp "${metrics_path}.tmp.XXXXXX")"
    jq --argjson rss "$rss" --argjson pss "$pss" \
        --argjson shared_clean "$shared_clean" --argjson shared_dirty "$shared_dirty" \
        --argjson guest_ram "$guest_ram" --argjson snapshot_backing "$snapshot_backing" \
        --arg sampled_at "$(date --utc +%Y-%m-%dT%H:%M:%SZ)" \
        '.vmm_rss_kb=$rss | .vmm_pss_kb=$pss | .vmm_shared_clean_kb=$shared_clean | .vmm_shared_dirty_kb=$shared_dirty | .guest_ram=$guest_ram | .snapshot_backing=$snapshot_backing | .sample_phase="explicit-post-fanout" | .sampled_at=$sampled_at' \
        "$metrics_path" > "$metrics_tmp"
    mv -f "$metrics_tmp" "$metrics_path"
    printf '%s\n' "$metrics_path"
}

_backend_cloud-hypervisor_fork_vm() {
    local snapshot_dir="$1"
    local child_prefix="$2"
    local count="$3"
    local memory_restore_mode="${4:-ondemand}"
    local resume_children="${5:-true}"
    local result_dir
    result_dir="$(mktemp -d)"
    local -a pids=() child_names=()
    local i child_name
    for i in $(seq 1 "$count"); do
        child_name="${child_prefix}-${i}"
        child_names+=("$child_name")
        (
            set -euo pipefail
            local child_disk cid metrics identity bootstrap_json metadata_path metadata_tmp
            child_disk="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$child_name/$child_name.qcow2"
            identity="$child_name"
            if declare -F bootstrap_json_for_child >/dev/null 2>&1; then
                bootstrap_json="$(bootstrap_json_for_child "$child_name")"
                if [[ -n "$bootstrap_json" ]]; then
                    identity="$(bootstrap_json_field "$bootstrap_json" instance_id)"
                    [[ -n "$identity" ]] || identity="$child_name"
                fi
            fi
            if declare -F allocate_cid_for_vm >/dev/null 2>&1; then
                cid="$(allocate_cid_for_vm "$child_name" "$identity")"
            else
                cid="$((200 + i))"
            fi
            metrics="$(_backend_cloud-hypervisor_restore_vm "$child_name" "$snapshot_dir" "$child_disk" "$cid" "$memory_restore_mode" "$resume_children")"
            metadata_path="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$child_name/enroll-on-restore.json"
            if [[ ! -f "$metadata_path" ]]; then
                metadata_tmp="$(mktemp "${metadata_path}.tmp.XXXXXX")"
                jq -n --arg instance "$identity" --arg name "$child_name" \
                    --arg snapshot "$(basename "$snapshot_dir")" --argjson cid "$cid" \
                    '{instance_id:$instance,vm_name:$name,source_snapshot:$snapshot,fresh_vsock_cid:$cid,fresh_enrollment_required:true,fresh_mtls_identity_required:true}' \
                    > "$metadata_tmp"
                chmod 600 "$metadata_tmp"
                mv -f "$metadata_tmp" "$metadata_path"
            fi
            jq -n --arg name "$child_name" --arg disk "$child_disk" --arg metrics "$metrics" \
                --arg instance "$identity" --argjson cid "$cid" \
                '{name:$name,instance_id:$instance,vsock_cid:$cid,disk:$disk,metrics:$metrics}' \
                > "$result_dir/$i.json"
        ) >"$result_dir/$i.stdout" 2>"$result_dir/$i.stderr" &
        pids+=("$!")
    done
    local failed=false
    for i in "${!pids[@]}"; do
        if ! wait "${pids[$i]}"; then
            failed=true
            log_error "fork child ${child_names[$i]} failed: $(sed -n '1,20p' "$result_dir/$((i + 1)).stderr")"
        fi
    done
    if [[ "$failed" == true ]]; then
        for child_name in "${child_names[@]}"; do
            if [[ -f "${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$child_name/cloud-hypervisor/vm.env" ]]; then
                _backend_cloud-hypervisor_destroy_vm "$child_name" || true
            fi
        done
        find "$result_dir" -type f -delete
        rmdir "$result_dir" 2>/dev/null || true
        return 1
    fi

    # Re-sample only after every child exists so the RAM-sharing evidence is
    # synchronized across the complete fan-out rather than captured during a
    # serial ramp-up.
    for i in $(seq 1 "$count"); do
        local metrics_path
        metrics_path="$(jq -r '.metrics' "$result_dir/$i.json")"
        _backend_cloud-hypervisor_sample_vm_memory "$(jq -r '.child_vm' "$metrics_path")" >/dev/null
        local metrics_tmp
        metrics_tmp="$(mktemp "${metrics_path}.tmp.XXXXXX")"
        jq '.sample_phase="post-fanout"' "$metrics_path" > "$metrics_tmp"
        mv -f "$metrics_tmp" "$metrics_path"
    done
    jq -cs 'sort_by(.name)' "$result_dir"/*.json
    find "$result_dir" -type f -delete
    rmdir "$result_dir" 2>/dev/null || true
}

# shellcheck disable=SC2153 # VM metadata variables are loaded from vm.env.
_backend_cloud-hypervisor_stop_vm() {
    local vm_name="$1"
    local state_file
    state_file="$(_ch_state_file "$vm_name")"
    [[ -f "$state_file" ]] || return 0
    # shellcheck source=/dev/null
    source "$state_file"

    if [[ -S "$API_SOCKET" ]]; then
        timeout 2 "$_ch_remote_bin" --api-socket "$API_SOCKET" shutdown >/dev/null 2>&1 || true
    fi
    if [[ -f "$PID_FILE" ]] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
        local pid
        pid="$(cat "$PID_FILE")"
        local _
        for _ in $(seq 1 50); do
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.1
        done
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            for _ in $(seq 1 20); do
                kill -0 "$pid" 2>/dev/null || break
                sleep 0.1
            done
        fi
        if kill -0 "$pid" 2>/dev/null; then
            kill -KILL "$pid" 2>/dev/null || true
            for _ in $(seq 1 20); do
                kill -0 "$pid" 2>/dev/null || break
                sleep 0.1
            done
        fi
        if kill -0 "$pid" 2>/dev/null; then
            log_error "Cloud Hypervisor process did not exit: $pid"
            return 1
        fi
    fi
}

# shellcheck disable=SC2153 # VM metadata variables are loaded from vm.env.
_backend_cloud-hypervisor_destroy_vm() {
    local vm_name="$1"
    # A VM that cannot be stopped must retain every VFIO recovery artifact.
    # Likewise, a failed GPU reset/rebind quarantines the group and must not be
    # masked by the non-GPU cleanup that follows.
    _backend_cloud-hypervisor_stop_vm "$vm_name" || return 1
    local state_file
    state_file="$(_ch_state_file "$vm_name")"
    if [[ -f "$state_file" ]]; then
        # shellcheck source=/dev/null
        source "$state_file"
        sudo ip link del "$TAP_NAME" 2>/dev/null || true
        find "$(_ch_state_dir "$vm_name")" -name '*.virtiofsd.pid' -type f -print0 2>/dev/null \
            | while IFS= read -r -d '' pid_file; do
                kill "$(cat "$pid_file")" 2>/dev/null || true
            done
        rm -f "$API_SOCKET" "$VSOCK_SOCKET" "$(_ch_state_dir "$vm_name")"/*.sock 2>/dev/null || true
        if _ch_truthy "${GPU_ENABLED:-false}"; then
            _ch_release_vfio_group "$vm_name" "$GPU_PCI_DEVICE" \
                "${VFIO_DEVICES_FILE:-$(_ch_state_dir "$vm_name")/vfio-devices.tsv}" \
                "${VFIO_GROUP_FILE:-$(_ch_state_dir "$vm_name")/vfio-group}" || return 1
        fi
        local share_path
        for share_path in "${INBOX_PATH:-}" "${OUTBOX_PATH:-}"; do
            [[ -n "$share_path" ]] || continue
            case "$share_path" in
                "${AGENTSHARE_ROOT:-/srv/agentshare}/"*)
                    rm -rf -- "$share_path" 2>/dev/null \
                        || sudo -n rm -rf -- "$share_path"
                    ;;
                *) log_warn "refusing to remove agentshare path outside configured root: $share_path" ;;
            esac
        done
        if [[ -n "${CID_REGISTRY:-}" && -f "$CID_REGISTRY" && -n "${VSOCK_CID:-}" ]]; then
            local cid_tmp
            cid_tmp="$(mktemp "${CID_REGISTRY}.tmp.XXXXXX")"
            grep -v "^${VSOCK_CID}=" "$CID_REGISTRY" > "$cid_tmp" || true
            mv -f "$cid_tmp" "$CID_REGISTRY"
        fi
        if [[ -n "${IP_REGISTRY:-}" && -f "$IP_REGISTRY" ]]; then
            local ip_tmp
            ip_tmp="$(mktemp "${IP_REGISTRY}.tmp.XXXXXX")"
            grep -v -e "^${vm_name}=" -e "=${vm_name}$" "$IP_REGISTRY" > "$ip_tmp" || true
            mv -f "$ip_tmp" "$IP_REGISTRY"
        fi
    fi
    local vm_dir="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$vm_name"
    case "$vm_dir" in
        "${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/"*) rm -rf -- "$vm_dir" ;;
        *) log_error "refusing to remove VM path outside storage root: $vm_dir"; return 1 ;;
    esac
}

_backend_cloud-hypervisor_get_vm_ip() {
    local vm_name="$1"
    local timeout="${2:-60}"
    local ip=""
    local start_time
    start_time="$(date +%s)"

    while true; do
        if [[ -f "${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$vm_name/vm-info.json" ]]; then
            ip="$(sed -n 's/.*"ip"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$vm_name/vm-info.json" | head -n1)"
        fi
        if [[ -z "$ip" && -f "${IP_REGISTRY:-}" ]]; then
            ip="$(grep "^$vm_name=" "$IP_REGISTRY" 2>/dev/null | cut -d= -f2)"
        fi
        if [[ -n "$ip" ]]; then
            echo "$ip"
            return 0
        fi
        [[ $(( $(date +%s) - start_time )) -lt "$timeout" ]] || return 1
        sleep 2
    done
}

_backend_cloud-hypervisor_add_dhcp() {
    return 0
}

_backend_cloud-hypervisor_set_autostart() {
    log_warn "Cloud Hypervisor backend does not support autostart yet"
}

_backend_cloud-hypervisor_vm_exists() {
    local vm_name="$1"
    local state_file
    state_file="$(_ch_state_file "$vm_name")"
    [[ -f "$state_file" ]]
}

_backend_cloud-hypervisor_attach_cloud_init() {
    log_warn "Cloud Hypervisor cloud-init reattach requires VM recreation"
    return 1
}

_backend_cloud-hypervisor_configure_virtiofs() {
    log_warn "Cloud Hypervisor virtiofs changes require VM recreation"
    return 1
}

_backend_cloud-hypervisor_console_hint() {
    local vm_name="$1"
    echo "tail -f $(_ch_state_dir "$vm_name")/serial.log"
}

_backend_cloud-hypervisor_start_hint() {
    local vm_name="$1"
    echo "AGENTIC_BACKEND=cloud-hypervisor bash -lc 'source images/qemu/provision-vm.sh; backend_start_vm $vm_name'"
}

_backend_cloud-hypervisor_stop_hint() {
    local vm_name="$1"
    echo "ch-remote --api-socket $(_ch_state_dir "$vm_name")/api.sock shutdown"
}

_backend_cloud-hypervisor_force_hint() {
    local vm_name="$1"
    echo "kill \$(cat $(_ch_state_dir "$vm_name")/pid)"
}

_backend_cloud-hypervisor_delete_hint() {
    local vm_name="$1"
    local vm_dir="$2"
    echo "AGENTIC_BACKEND=cloud-hypervisor scripts/destroy-vm.sh $vm_name --force && rm -rf $vm_dir"
}
