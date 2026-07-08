#!/bin/bash
# lib/network.sh - VM network addressing and libvirt DHCP reservation management
#
# Provides functions for:
#   - Deterministic MAC address generation from VM name
#   - Static IP allocation and registry management
#   - VSock CID allocation and registry management
#   - libvirt DHCP reservation add/remove
#
# Required globals (validated at source time):
#   IP_REGISTRY   - Path to the IP allocation registry file
#   IP_BASE       - Network base (e.g., 192.168.122)
#   IP_START      - First usable host octet in range
#   IP_END        - Last usable host octet in range

CID_REGISTRY="${CID_REGISTRY:-${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/.vsock-cid-registry}"
CID_START="${CID_START:-3}"
CID_END="${CID_END:-65535}"

: "${IP_REGISTRY:?lib/network.sh requires IP_REGISTRY}"
: "${IP_BASE:?lib/network.sh requires IP_BASE}"
: "${IP_START:?lib/network.sh requires IP_START}"
: "${IP_END:?lib/network.sh requires IP_END}"

# Generate deterministic MAC address from VM name
# Format: 52:54:00:XX:XX:XX where XX comes from hash of name
generate_mac_from_name() {
    local name="$1"
    local hash
    hash=$(echo -n "$name" | md5sum | cut -c1-6)
    local b1="${hash:0:2}"
    local b2="${hash:2:2}"
    local b3="${hash:4:2}"
    echo "52:54:00:$b1:$b2:$b3"
}

# Allocate a static IP for VM (deterministic based on name pattern or registry)
allocate_ip_for_vm() {
    local vm_name="$1"
    local network="$2"

    # Ensure registry exists
    mkdir -p "$(dirname "$IP_REGISTRY")"
    touch "$IP_REGISTRY" 2>/dev/null || sudo touch "$IP_REGISTRY"

    # Check if this VM already has an IP
    local existing
    existing=$(grep "^$vm_name=" "$IP_REGISTRY" 2>/dev/null | cut -d= -f2)
    if [[ -n "$existing" ]]; then
        echo "$existing"
        return 0
    fi

    # Try pattern-based allocation (agent-01 → .201, agent-02 → .202, etc.)
    if [[ "$vm_name" =~ ^agent-([0-9]+)$ ]]; then
        local num="${BASH_REMATCH[1]}"
        num=$((10#$num))  # Remove leading zeros
        if [[ $num -ge 1 && $num -le 54 ]]; then
            local ip="$IP_BASE.$((IP_START + num - 1))"
            echo "$vm_name=$ip" >> "$IP_REGISTRY"
            echo "$ip"
            return 0
        fi
    fi

    # Find next available IP in range
    for i in $(seq $IP_START $IP_END); do
        local candidate="$IP_BASE.$i"
        if ! grep -q "=$candidate$" "$IP_REGISTRY" 2>/dev/null; then
            echo "$vm_name=$candidate" >> "$IP_REGISTRY"
            echo "$candidate"
            return 0
        fi
    done

    log_error "No available IPs in range $IP_BASE.$IP_START-$IP_END"
    return 1
}

# Path to the lockfile guarding mutations of the CID registry. Concurrent
# provisioners (#581) must serialize the read-check-append critical section so
# two VMs never claim the same CID or lose an allocation to a racing append.
_cid_registry_lockfile() {
    printf '%s.lock' "$CID_REGISTRY"
}

_ensure_cid_registry_path_writable() {
    local path="$1"
    local owner
    owner="$(id -u):$(id -g)"

    touch "$path" 2>/dev/null || sudo touch "$path"
    chmod u+rw "$path" 2>/dev/null || sudo chmod u+rw "$path" 2>/dev/null || true
    if [[ ! -w "$path" ]]; then
        sudo chown "$owner" "$path" 2>/dev/null || true
    fi
    if [[ ! -w "$path" ]]; then
        log_error "CID registry path is not writable by $(id -un): $path"
        return 1
    fi
}

_vm_info_instance_id() {
    local vm_name="$1"
    local vm_info_file="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}/$vm_name/vm-info.json"

    if [[ -f "$vm_info_file" ]]; then
        sed -n 's/.*"instance_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$vm_info_file" | head -n1
        return 0
    fi
    return 1
}

# Inner allocation logic — runs while the CID registry lock is held. Echoes the
# allocated CID to stdout; the caller is responsible for serialization.
_allocate_cid_for_vm_locked() {
    local vm_name="$1"
    local instance_id="${2:-$vm_name}"

    # Check if this VM already has a CID
    local existing
    existing=$(awk -F= -v vm="$vm_name" -v id="$instance_id" '
        $1 ~ /^[0-9]+$/ && ($2 == id || $2 == vm) { print $1; exit }
        ($1 == id || $1 == vm) && $2 ~ /^[0-9]+$/ { print $2; exit }
    ' "$CID_REGISTRY" 2>/dev/null || true)
    if [[ -n "$existing" ]]; then
        echo "$existing"
        return 0
    fi

    # Try pattern-based allocation (agent-01 → first CID, agent-02 → second CID, ...)
    if [[ "$vm_name" =~ ^agent-([0-9]+)$ ]]; then
        local num="${BASH_REMATCH[1]}"
        num=$((10#$num))
        local preferred
        preferred=$((CID_START + num - 1))
        if [[ $preferred -ge $CID_START && $preferred -le $CID_END ]]; then
            if ! awk -F= -v cid="$preferred" '($1 == cid || $2 == cid) { found=1 } END { exit(found ? 0 : 1) }' \
                    "$CID_REGISTRY" 2>/dev/null; then
                echo "$preferred=$instance_id" >> "$CID_REGISTRY"
                echo "$preferred"
                return 0
            fi
        fi
    fi

    # Find next available CID in range (first-fit)
    local candidate
    for candidate in $(seq "$CID_START" "$CID_END"); do
        if ! awk -F= -v cid="$candidate" '($1 == cid || $2 == cid) { found=1 } END { exit(found ? 0 : 1) }' \
                "$CID_REGISTRY" 2>/dev/null; then
            echo "$candidate=$instance_id" >> "$CID_REGISTRY"
            echo "$candidate"
            return 0
        fi
    done

    log_error "No available VSock CIDs in range $CID_START-$CID_END"
    return 1
}

# Allocate a unique VSock CID for VM (deterministic based on name pattern or
# registry). Serialized with flock so parallel provisioning cannot duplicate or
# lose CIDs (#581).
allocate_cid_for_vm() {
    local vm_name="$1"
    local instance_id="${2:-${AGENT_INSTANCE_ID:-${AIWG_INSTANCE_ID:-$vm_name}}}"

    # Ensure registry + lockfile exist
    mkdir -p "$(dirname "$CID_REGISTRY")"
    _ensure_cid_registry_path_writable "$CID_REGISTRY"
    local lockfile
    lockfile="$(_cid_registry_lockfile)"
    _ensure_cid_registry_path_writable "$lockfile"

    if command -v flock >/dev/null 2>&1; then
        # FD 9 holds the exclusive lock for the lifetime of the subshell; the
        # inner function's stdout (the CID) propagates to this function's stdout.
        (
            if ! flock -w 30 9; then
                log_error "Timed out acquiring CID registry lock ($lockfile)"
                exit 1
            fi
            _allocate_cid_for_vm_locked "$vm_name" "$instance_id"
        ) 9>"$lockfile"
    else
        # flock unavailable (non-Linux/minimal env): best-effort, unserialized.
        _allocate_cid_for_vm_locked "$vm_name" "$instance_id"
    fi
}

# Add DHCP reservation to libvirt network
add_dhcp_reservation() {
    local network="$1"
    local vm_name="$2"
    local mac="$3"
    local ip="$4"

    # Check if reservation already exists
    if virsh_cmd net-dumpxml "$network" 2>/dev/null | grep -q "mac='$mac'"; then
        log_info "DHCP reservation for $mac already exists"
        return 0
    fi

    # Add the host entry to the network
    # Note: DHCP reservation failure is non-fatal since we use cloud-init for static IP
    if virsh_cmd net-update "$network" add ip-dhcp-host \
        "<host mac='$mac' name='$vm_name' ip='$ip'/>" \
        --live --config 2>/dev/null; then
        log_success "DHCP reservation added"
    else
        log_warn "Could not add DHCP reservation (may need network restart)"
        log_info "Continuing - cloud-init will configure static IP"
    fi

    return 0
}

# Remove DHCP reservation from libvirt network
remove_dhcp_reservation() {
    local network="$1"
    local vm_name="$2"
    local mac="$3"
    local ip="$4"

    virsh_cmd net-update "$network" delete ip-dhcp-host \
        "<host mac='$mac' name='$vm_name' ip='$ip'/>" \
        --live --config 2>/dev/null || true

    # Remove from registry
    sed -i "/^$vm_name=/d" "$IP_REGISTRY" 2>/dev/null || true
}

# Remove CID allocation for VM. Serialized with flock against concurrent
# allocate/remove so a racing append is never clobbered (#581).
remove_cid_allocation() {
    local vm_name="$1"
    [[ -f "$CID_REGISTRY" ]] || return 0
    local instance_id
    instance_id="$(_vm_info_instance_id "$vm_name" || true)"
    local lockfile
    lockfile="$(_cid_registry_lockfile)"
    touch "$lockfile" 2>/dev/null || sudo touch "$lockfile" 2>/dev/null || true

    if command -v flock >/dev/null 2>&1; then
        (
            flock -w 30 9 || exit 1
            if [[ -n "$instance_id" ]]; then
                sed -i -e "/^$vm_name=/d" -e "/=$vm_name$/d" -e "/^$instance_id=/d" -e "/=$instance_id$/d" "$CID_REGISTRY" 2>/dev/null || true
            else
                sed -i -e "/^$vm_name=/d" -e "/=$vm_name$/d" "$CID_REGISTRY" 2>/dev/null || true
            fi
        ) 9>"$lockfile"
    else
        if [[ -n "$instance_id" ]]; then
            sed -i -e "/^$vm_name=/d" -e "/=$vm_name$/d" -e "/^$instance_id=/d" -e "/=$instance_id$/d" "$CID_REGISTRY" 2>/dev/null || true
        else
            sed -i -e "/^$vm_name=/d" -e "/=$vm_name$/d" "$CID_REGISTRY" 2>/dev/null || true
        fi
    fi
}

# Get pre-allocated IP for a VM (returns empty if not allocated)
get_vm_allocated_ip() {
    local vm_name="$1"
    grep "^$vm_name=" "$IP_REGISTRY" 2>/dev/null | cut -d= -f2
}

# Get pre-allocated VSock CID for a VM (returns empty if not allocated)
get_vm_allocated_cid() {
    local vm_name="$1"
    local instance_id
    instance_id="$(_vm_info_instance_id "$vm_name" || true)"
    awk -F= -v vm="$vm_name" -v id="$instance_id" '
        $1 ~ /^[0-9]+$/ && ($2 == vm || (id != "" && $2 == id)) { print $1; exit }
        ($1 == vm || (id != "" && $1 == id)) && $2 ~ /^[0-9]+$/ { print $2; exit }
    ' "$CID_REGISTRY" 2>/dev/null || true
}
