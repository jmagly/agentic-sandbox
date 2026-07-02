#!/bin/bash
# Clean stale VM-backed E2E substrate left by interrupted CI runs.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/reap-e2e-vms.sh [OPTIONS]

Options:
  --current NAME       Keep the VM for the currently running E2E lane.
  --network NAME       Libvirt network to clean (default: default).
  --vm-root PATH       VM storage root (default: /var/lib/agentic-sandbox/vms).
  --ip-registry PATH   IP registry file (default: <vm-root>/.ip-registry).
  --cid-registry PATH  VSock CID registry file (default: <vm-root>/.vsock-cid-registry).
  --virsh-uri URI      Libvirt URI to use (default: qemu:///system).
  --virsh-timeout SEC  Maximum seconds for each virsh call (default: 15).
  --dry-run            Print actions without mutating the host.
  --skip-libvirt       Skip libvirt domain and DHCP cleanup.
  -h, --help           Show this help.
EOF
}

parse_args() {
    HELP_REQUESTED=0

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --current)
                CURRENT_VM="${2:?--current requires a VM name}"
                shift 2
                ;;
            --network)
                NETWORK="${2:?--network requires a network name}"
                shift 2
                ;;
            --vm-root)
                VM_ROOT="${2:?--vm-root requires a path}"
                shift 2
                ;;
            --ip-registry)
                IP_REGISTRY="${2:?--ip-registry requires a path}"
                shift 2
                ;;
            --cid-registry)
                CID_REGISTRY="${2:?--cid-registry requires a path}"
                shift 2
                ;;
            --virsh-uri)
                VIRSH_URI="${2:?--virsh-uri requires a URI}"
                shift 2
                ;;
            --virsh-timeout)
                VIRSH_TIMEOUT="${2:?--virsh-timeout requires a number of seconds}"
                shift 2
                ;;
            --dry-run)
                DRY_RUN=1
                shift
                ;;
            --skip-libvirt)
                SKIP_LIBVIRT=1
                shift
                ;;
            -h|--help)
                usage
                HELP_REQUESTED=1
                return 0
                ;;
            *)
                echo "Unknown option: $1" >&2
                usage >&2
                return 2
                ;;
        esac
    done

    IP_REGISTRY="${IP_REGISTRY:-$VM_ROOT/.ip-registry}"
    CID_REGISTRY="${CID_REGISTRY:-$VM_ROOT/.vsock-cid-registry}"

    return 0
}

run() {
    echo "[reaper] $*"
    if [[ "$DRY_RUN" == "0" ]]; then
        "$@"
    fi
}

virsh_cmd() {
    if command -v timeout >/dev/null 2>&1; then
        timeout "$VIRSH_TIMEOUT" virsh -c "$VIRSH_URI" "$@"
    else
        virsh -c "$VIRSH_URI" "$@"
    fi
}

is_e2e_vm() {
    [[ "$1" =~ ^agentic-e2e-[0-9]+$ ]]
}

keep_current() {
    [[ -n "$CURRENT_VM" && "$1" == "$CURRENT_VM" ]]
}

vm_info_vsock_cid() {
    local vm="$1"
    local vm_info_file="$VM_ROOT/$vm/vm-info.json"

    if [[ -f "$vm_info_file" ]]; then
        sed -n 's/.*"vsock_cid"[[:space:]]*:[[:space:]]*"\([0-9][0-9]*\)".*/\1/p' "$vm_info_file" | head -n1
        return 0
    fi
    return 1
}

vm_info_instance_id() {
    local vm="$1"
    local vm_info_file="$VM_ROOT/$vm/vm-info.json"

    if [[ -f "$vm_info_file" ]]; then
        sed -n 's/.*"instance_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$vm_info_file" | head -n1
        return 0
    fi
    return 1
}

vm_for_instance_id() {
    local instance_id="$1"
    local vm vm_id

    [[ -n "$instance_id" ]] || return 1
    for dir in "$VM_ROOT"/agentic-e2e-*; do
        [[ -d "$dir" ]] || continue
        vm="$(basename "$dir")"
        vm_id="$(vm_info_instance_id "$vm" || true)"
        if [[ "$vm_id" == "$instance_id" ]]; then
            printf '%s\n' "$vm"
            return 0
        fi
    done
    return 1
}

is_vsock_subject_retained() {
    local vm="$1"

    # Keep CID rows for VM state we can still account for:
    # - the explicitly kept CURRENT VM
    # - domains still known to libvirt (skipped under --skip-libvirt so the
    #   reaper stays deterministic and never consults libvirt when the caller
    #   has explicitly opted out — e.g. CI/unit contexts)
    # - persisted vm-info records (e.g., stopped but not yet reaped)
    keep_current "$vm" && return 0
    if [[ "${SKIP_LIBVIRT:-0}" != "1" ]] \
        && command -v virsh >/dev/null 2>&1 \
        && virsh_cmd dominfo "$vm" &>/dev/null; then
        return 0
    fi
    [[ -f "$VM_ROOT/$vm/vm-info.json" ]] && return 0
    return 1
}

reap_domain() {
    local vm="$1"

    if keep_current "$vm"; then
        echo "[reaper] keeping current run VM $vm"
        return
    fi

    echo "::notice::Reaping stale E2E VM $vm"
    if virsh_cmd domstate "$vm" 2>/dev/null | grep -qi '^running$'; then
        run virsh_cmd destroy "$vm"
    fi
    run virsh_cmd undefine "$vm" --nvram --remove-all-storage \
        || run virsh_cmd undefine "$vm" \
        || true
}

reap_vm_dir() {
    local vm="$1"
    local vm_dir="$VM_ROOT/$vm"

    if keep_current "$vm"; then
        return
    fi
    if [[ -d "$vm_dir" ]]; then
        local vm_root_real vm_dir_real
        vm_root_real="$(realpath -m "$VM_ROOT")"
        vm_dir_real="$(realpath -m "$vm_dir")"
        [[ "$vm_dir_real" == "$vm_root_real/$vm" ]] || return
        run rm -rf "$vm_dir"
    fi
}

reap_dhcp_reservations() {
    local xml host_line name mac ip removed=0
    xml="$(virsh_cmd net-dumpxml "$NETWORK" 2>/dev/null || true)"
    [[ -n "$xml" ]] || return 0

    while IFS= read -r host_line; do
        name="$(grep -oP "name='\K[^']+" <<<"$host_line" || true)"
        [[ -n "$name" ]] || continue
        is_e2e_vm "$name" || continue
        keep_current "$name" && continue

        mac="$(grep -oP "mac='\K[^']+" <<<"$host_line" || true)"
        ip="$(grep -oP "ip='\K[^']+" <<<"$host_line" || true)"
        if [[ -n "$mac" && -n "$ip" ]]; then
            run virsh_cmd net-update "$NETWORK" delete ip-dhcp-host \
                "<host mac='$mac' name='$name' ip='$ip'/>" \
                --live --config || true
            removed=1
        fi
    done < <(grep "<host " <<<"$xml" || true)

    if [[ "$removed" == "0" ]]; then
        echo "[reaper] no stale E2E DHCP reservations found"
    fi
}

reap_ip_registry() {
    [[ -f "$IP_REGISTRY" ]] || {
        echo "[reaper] IP registry missing: $IP_REGISTRY"
        return 0
    }

    local tmp
    tmp="$(mktemp)"
    awk -F= -v current="$CURRENT_VM" '
        $1 ~ /^agentic-e2e-[0-9]+$/ && $1 != current { next }
        { print }
    ' "$IP_REGISTRY" > "$tmp"

    if cmp -s "$IP_REGISTRY" "$tmp"; then
        echo "[reaper] no stale E2E IP registry rows found"
        rm -f "$tmp"
        return 0
    fi

    if [[ "$DRY_RUN" == "1" ]]; then
        echo "[reaper] would update $IP_REGISTRY"
        rm -f "$tmp"
    else
        cat "$tmp" > "$IP_REGISTRY"
        rm -f "$tmp"
        echo "[reaper] removed stale E2E IP registry rows from $IP_REGISTRY"
    fi
}

reap_cid_registry() {
    [[ -f "$CID_REGISTRY" ]] || {
        echo "[reaper] VSock CID registry missing: $CID_REGISTRY"
        return 0
    }

    local tmp
    tmp="$(mktemp)"
    local changed=0
    local parsed_vm parsed_cid parsed_identity vm_info_cid vm_info_id line_left line_right
    while IFS= read -r line || [[ -n "$line" ]]; do
        [[ -n "$line" ]] || continue
        if [[ "$line" =~ ^[[:space:]]*$ ]]; then
            continue
        fi
        if [[ ! "$line" =~ ^([^=]+)=([^=]+)$ ]]; then
            echo "[reaper] removing malformed CID registry entry: $line"
            changed=1
            continue
        fi

        line_left="${BASH_REMATCH[1]}"
        line_right="${BASH_REMATCH[2]}"
        if [[ "$line_left" =~ ^[0-9]+$ ]]; then
            parsed_cid="$line_left"
            parsed_identity="$line_right"
            parsed_vm="$parsed_identity"
            if ! is_e2e_vm "$parsed_vm"; then
                parsed_vm="$(vm_for_instance_id "$parsed_identity" || true)"
            fi
        elif [[ "$line_right" =~ ^[0-9]+$ ]]; then
            parsed_vm="$line_left"
            parsed_identity="$line_left"
            parsed_cid="$line_right"
        else
            echo "[reaper] removing malformed CID registry entry: $line"
            changed=1
            continue
        fi

        if [[ -z "$parsed_vm" ]]; then
            printf '%s\n' "$line" >> "$tmp"
            continue
        fi
        if ! is_e2e_vm "$parsed_vm"; then
            printf '%s\n' "$line" >> "$tmp"
            continue
        fi
        if ! keep_current "$parsed_vm"; then
            echo "[reaper] removing stale CID registry row: $parsed_vm"
            changed=1
            continue
        fi
        if ! is_vsock_subject_retained "$parsed_vm"; then
            echo "[reaper] removing stale CID registry row: $parsed_vm"
            changed=1
            continue
        fi

        vm_info_cid=$(vm_info_vsock_cid "$parsed_vm" || true)
        vm_info_id=$(vm_info_instance_id "$parsed_vm" || true)
        if [[ -n "$vm_info_cid" && "$vm_info_cid" != "$parsed_cid" ]]; then
            echo "[reaper] reconciling CID mismatch for $parsed_vm: registry=$parsed_cid vm-info=$vm_info_cid"
            parsed_cid="$vm_info_cid"
            changed=1
        fi
        if [[ -n "$vm_info_id" ]]; then
            printf '%s=%s\n' "$parsed_cid" "$vm_info_id" >> "$tmp"
        else
            printf '%s=%s\n' "$parsed_vm" "$parsed_cid" >> "$tmp"
        fi
    done < "$CID_REGISTRY"

    if [[ "$changed" == "0" ]] && cmp -s "$CID_REGISTRY" "$tmp"; then
        echo "[reaper] no stale E2E CID registry rows found"
        rm -f "$tmp"
        return 0
    fi

    if [[ "$DRY_RUN" == "1" ]]; then
        echo "[reaper] would update $CID_REGISTRY"
        rm -f "$tmp"
    else
        cat "$tmp" > "$CID_REGISTRY"
        rm -f "$tmp"
        echo "[reaper] removed stale E2E CID registry rows from $CID_REGISTRY"
    fi
}

main() {
    NETWORK="${NETWORK:-default}"
    VM_ROOT="${VM_ROOT:-/var/lib/agentic-sandbox/vms}"
    IP_REGISTRY="${IP_REGISTRY:-}"
    CID_REGISTRY="${CID_REGISTRY:-}"
    CURRENT_VM="${CURRENT_VM:-}"
    VIRSH_URI="${VIRSH_URI:-qemu:///system}"
    VIRSH_TIMEOUT="${VIRSH_TIMEOUT:-15}"
    DRY_RUN=0
    SKIP_LIBVIRT=0
    HELP_REQUESTED=0

    parse_args "$@"
    local parse_status=$?
    if [[ "$parse_status" -ne 0 ]]; then
        return "$parse_status"
    fi
    if [[ "$HELP_REQUESTED" == "1" ]]; then
        return 0
    fi

    local found=0

    # Reconcile CID rows before deleting VM directories. Canonical rows are
    # `cid=instance_id`, so vm-info is needed to map them back to E2E VM names.
    reap_cid_registry

    if [[ "$SKIP_LIBVIRT" == "1" ]]; then
        echo "[reaper] libvirt cleanup skipped"
    elif command -v virsh >/dev/null 2>&1; then
        while IFS= read -r vm; do
            [[ -n "$vm" ]] || continue
            is_e2e_vm "$vm" || continue
            found=1
            reap_domain "$vm"
            reap_vm_dir "$vm"
        done < <(virsh_cmd list --all --name | grep -E '^agentic-e2e-[0-9]+$' || true)

        reap_dhcp_reservations
    else
        echo "[reaper] virsh unavailable; skipping libvirt domain and DHCP cleanup"
    fi

    if [[ -d "$VM_ROOT" ]]; then
        while IFS= read -r vm_dir; do
            vm="$(basename "$vm_dir")"
            is_e2e_vm "$vm" || continue
            found=1
            reap_vm_dir "$vm"
        done < <(find "$VM_ROOT" -maxdepth 1 -mindepth 1 -type d -name 'agentic-e2e-*' 2>/dev/null || true)
    fi

    reap_ip_registry

    if [[ "$found" == "0" ]]; then
        echo "[reaper] no stale E2E VMs found"
    fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
