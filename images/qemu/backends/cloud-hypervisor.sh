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
    date +%s%3N
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
        '
        .disks[0].path = $disk
        | .disks[0].backing_files = true
        | if ($cloud_init != "" and (.disks | length) > 1) then .disks[1].path = $cloud_init else . end
        | .net[0].tap = $tap
        | .net[0].mac = $mac
        | del(.net[0].host_mac)
        | .vsock.cid = $cid
        | .vsock.socket = $vsock
        | .serial.file = $serial
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
    if [[ -n "$gpu_config_path" ]]; then
        log_warn "GPU passthrough is not implemented for Cloud Hypervisor yet; use libvirt until CH-8"
    fi

    local state_dir
    state_dir="$(_ch_state_dir "$vm_name")"
    mkdir -p "$state_dir"

    local api_socket="$state_dir/api.sock"
    local pid_file="$state_dir/pid"
    local serial_log="$state_dir/serial.log"
    local tap
    tap="$(_ch_tap_name "$vm_name")"
    local bridge
    bridge="$(_ch_bridge_for_network "$network")"
    local vsock_socket="$state_dir/vsock.sock"

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
        "MEM_LIMIT_MB=$mem_limit_mb" \
        "CPU_QUOTA_PCT=$cpu_quota_pct" \
        "IO_WEIGHT=$io_weight" \
        "IO_READ_BPS=$io_read_bps" \
        "IO_WRITE_BPS=$io_write_bps"

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
        mkdir -p "$child_inbox"/{outputs,logs,runs}
        mkdir -p "$child_outbox"/{progress,artifacts}
        chmod 777 "$child_inbox" "$child_outbox" 2>/dev/null || true
        chmod 755 "$child_inbox"/{outputs,logs,runs} "$child_outbox"/{progress,artifacts} 2>/dev/null || true
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

    _backend_cloud-hypervisor_prepare_network "$NETWORK" "$VM_NAME" "$MAC_ADDRESS" ""

    local state_dir
    state_dir="$(_ch_state_dir "$VM_NAME")"
    rm -f "$API_SOCKET" "$VSOCK_SOCKET"
    : > "$SERIAL_LOG"

    local -a fs_args=()
    _ch_build_fs_args "$state_dir" "$USE_AGENTSHARE" "$INBOX_PATH" "$OUTBOX_PATH" "${CARBONYL_SESSION_PATH:-}" fs_args

    local -a cmd=(
        "$_ch_bin"
        --api-socket "$API_SOCKET"
        --kernel "$_ch_firmware"
        --disk "path=$DISK_PATH,image_type=qcow2"
        --disk "path=$CLOUD_INIT_ISO,readonly=on"
        --cpus "boot=$CPUS"
        --memory "size=${MEMORY_MB}M,shared=on"
        --net "tap=$TAP_NAME,mac=$MAC_ADDRESS"
        --vsock "cid=$VSOCK_CID,socket=$VSOCK_SOCKET"
        --serial "file=$SERIAL_LOG"
        --console off
    )
    cmd+=("${fs_args[@]}")

    nohup "${cmd[@]}" >/dev/null 2>&1 &
    echo "$!" > "$PID_FILE"
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
    "$_ch_remote_bin" --api-socket "$API_SOCKET" pause
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

    _backend_cloud-hypervisor_prepare_network "$NETWORK" "$child_name" "$mac" "" >&2
    rm -f "$api_socket" "$vsock_socket"
    : > "$serial_log"
    : > "$vmm_log"

    local -a fs_args=()
    _ch_build_fs_args "$child_state_dir" "${USE_AGENTSHARE:-false}" "$child_inbox_path" "$child_outbox_path" "${CARBONYL_SESSION_PATH:-}" fs_args

    local source_url
    source_url="$(_ch_prepare_restore_source "$snapshot_dir" "$restore_source_dir" "$child_disk" "${CLOUD_INIT_ISO:-}" "$tap" "$mac" "$vsock_cid" "$vsock_socket" "$serial_log")"

    local start_ms end_ms
    start_ms="$(_ch_ms_now)"
    local -a cmd=(
        "$_ch_bin"
        --api-socket "$api_socket"
        --restore "source_url=$source_url,memory_restore_mode=$memory_restore_mode,resume=$resume"
    )
    cmd+=("${fs_args[@]}")
    nohup "${cmd[@]}" >"$vmm_log" 2>&1 &
    echo "$!" > "$pid_file"
    _ch_wait_for_api_ready "$api_socket" "$pid_file" "$vmm_log" "${AGENTIC_CH_RESTORE_API_WAIT_MS:-1000}" || return 1
    end_ms="$(_ch_ms_now)"
    local vmm_pid rss_kb pss_kb shared_clean_kb shared_dirty_kb
    vmm_pid="$(cat "$pid_file" 2>/dev/null || true)"
    rss_kb="$(_ch_proc_mem_kb "$vmm_pid" Rss)"
    pss_kb="$(_ch_proc_mem_kb "$vmm_pid" Pss)"
    shared_clean_kb="$(_ch_proc_mem_kb "$vmm_pid" Shared_Clean)"
    shared_dirty_kb="$(_ch_proc_mem_kb "$vmm_pid" Shared_Dirty)"
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
  "duration_ms": $((end_ms - start_ms))
}
EOF
    printf '%s\n' "$child_state_dir/restore-metrics.json"
}

_backend_cloud-hypervisor_fork_vm() {
    local snapshot_dir="$1"
    local child_prefix="$2"
    local count="$3"
    local memory_restore_mode="${4:-ondemand}"
    local -a children=()
    local i child_name child_disk cid metrics metrics_tmp
    for i in $(seq 1 "$count"); do
        child_name="${child_prefix}-${i}"
        child_disk="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$child_name/$child_name.qcow2"
        if declare -F allocate_cid_for_vm >/dev/null 2>&1; then
            cid="$(allocate_cid_for_vm "$child_name" "$child_name")"
        else
            cid="$((200 + i))"
        fi
        metrics_tmp="$(mktemp)"
        _backend_cloud-hypervisor_restore_vm "$child_name" "$snapshot_dir" "$child_disk" "$cid" "$memory_restore_mode" true > "$metrics_tmp"
        metrics="$(cat "$metrics_tmp")"
        rm -f "$metrics_tmp"
        cat > "${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$child_name/enroll-on-restore.json" <<EOF
{
  "instance_id": "$(_ch_json_escape "$child_name")",
  "vm_name": "$(_ch_json_escape "$child_name")",
  "source_snapshot": "$(_ch_json_escape "$(basename "$snapshot_dir")")",
  "fresh_vsock_cid": $cid,
  "fresh_enrollment_required": true,
  "fresh_mtls_identity_required": true
}
EOF
        children+=("{\"name\":\"$(_ch_json_escape "$child_name")\",\"vsock_cid\":$cid,\"disk\":\"$(_ch_json_escape "$child_disk")\",\"metrics\":\"$(_ch_json_escape "$metrics")\"}")
    done
    printf '[%s]\n' "$(IFS=,; echo "${children[*]}")"
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
        "$_ch_remote_bin" --api-socket "$API_SOCKET" shutdown >/dev/null 2>&1 || true
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
        fi
    fi
}

# shellcheck disable=SC2153 # VM metadata variables are loaded from vm.env.
_backend_cloud-hypervisor_destroy_vm() {
    local vm_name="$1"
    _backend_cloud-hypervisor_stop_vm "$vm_name"
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
    fi
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
