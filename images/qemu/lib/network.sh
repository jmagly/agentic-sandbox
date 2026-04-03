#!/bin/bash
# lib/network.sh - VM network addressing and libvirt DHCP reservation management
#
# Provides functions for:
#   - Deterministic MAC address generation from VM name
#   - Static IP allocation and registry management
#   - libvirt DHCP reservation add/remove
#
# Required globals (validated at source time):
#   IP_REGISTRY   - Path to the IP allocation registry file
#   IP_BASE       - Network base (e.g., 192.168.122)
#   IP_START      - First usable host octet in range
#   IP_END        - Last usable host octet in range

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

# Add DHCP reservation to libvirt network
add_dhcp_reservation() {
    local network="$1"
    local vm_name="$2"
    local mac="$3"
    local ip="$4"

    # Check if reservation already exists
    if virsh net-dumpxml "$network" 2>/dev/null | grep -q "mac='$mac'"; then
        log_info "DHCP reservation for $mac already exists"
        return 0
    fi

    # Add the host entry to the network
    # Note: DHCP reservation failure is non-fatal since we use cloud-init for static IP
    if virsh net-update "$network" add ip-dhcp-host \
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

    virsh net-update "$network" delete ip-dhcp-host \
        "<host mac='$mac' name='$vm_name' ip='$ip'/>" \
        --live --config 2>/dev/null || true

    # Remove from registry
    sed -i "/^$vm_name=/d" "$IP_REGISTRY" 2>/dev/null || true
}

# Get pre-allocated IP for a VM (returns empty if not allocated)
get_vm_allocated_ip() {
    local vm_name="$1"
    grep "^$vm_name=" "$IP_REGISTRY" 2>/dev/null | cut -d= -f2
}
