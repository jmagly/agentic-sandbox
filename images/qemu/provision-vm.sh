#!/bin/bash
# provision-vm.sh - Rapidly provision agent VMs from base images
#
# Creates overlay VMs that boot in seconds using qcow2 backing files.
# Injects SSH keys and cloud-init config for immediate access.
#
# Usage: ./provision-vm.sh [OPTIONS] NAME
# Example: ./provision-vm.sh --cpus 4 --memory 8G agent-01

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Source shared logging library if available, otherwise use builtin
LOGGING_LIB="$PROJECT_ROOT/scripts/lib/logging.sh"
if [[ -f "$LOGGING_LIB" && "${USE_SHARED_LOGGING:-true}" == "true" ]]; then
    # shellcheck source=../../scripts/lib/logging.sh
    source "$LOGGING_LIB"
    LOG_SCRIPT_NAME="provision-vm"
else
    # Fallback to inline logging
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    NC='\033[0m'
    log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
    log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
    log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
    log_error() { echo -e "${RED}[ERROR]${NC} $1" >&2; }
fi

# Default paths
BASE_IMAGES_DIR="${BASE_IMAGES_DIR:-/mnt/ops/base-images}"
VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
SSH_KEY_DIR="${SSH_KEY_DIR:-$HOME/.ssh}"
PROFILES_DIR="$SCRIPT_DIR/profiles"
IP_REGISTRY="$VM_STORAGE_DIR/.ip-registry"
CID_REGISTRY="$VM_STORAGE_DIR/.vsock-cid-registry"
# Canonical in-guest agent-client install path. Must match the baked-image
# location (build-base-image.sh) and agent-rs/systemd/agent-client.service so the
# image, live-deploy, and readiness-check paths never diverge (#573).
AGENT_CLIENT_BIN="${AGENT_CLIENT_BIN:-/opt/agentic-sandbox/bin/agent-client}"
AGENTSHARE_ROOT="${AGENTSHARE_ROOT:-/srv/agentshare}"
TASKS_ROOT="${TASKS_ROOT:-/srv/agentshare/tasks}"
SECRETS_DIR="${SECRETS_DIR:-/var/lib/agentic-sandbox/secrets}"
AGENT_TOKENS_FILE="$SECRETS_DIR/agent-tokens"
HEALTH_TOKENS_FILE="$SECRETS_DIR/health-tokens"
DEFAULT_NETWORK_MODE="full"  # Backwards compatible: isolated|allowlist|full
MANAGEMENT_SERVER="${MANAGEMENT_SERVER:-host.internal:8120}"
MANAGEMENT_HOST_IP="${MANAGEMENT_HOST_IP:-192.168.122.1}"
DEFAULT_VSOCK_HOST_CID="2"

# IP allocation range (for agent VMs)
IP_BASE="192.168.122"
IP_START=201
IP_END=254

# Default VM resources (sized for 2-4 concurrent VMs)
DEFAULT_CPUS="4"
DEFAULT_MEMORY="8G"
DEFAULT_DISK="40G"
DEFAULT_BASE="ubuntu-24.04"
DEFAULT_PROFILE=""  # Empty = basic provisioning

# Service account
SERVICE_USER="agent"
LIBVIRT_QEMU_GROUP="${LIBVIRT_QEMU_GROUP:-}"

# Colors (used by profile scripts and inline fallback)
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Source library files
# shellcheck source=lib/secrets.sh
source "$SCRIPT_DIR/lib/secrets.sh"
# shellcheck source=lib/network.sh
source "$SCRIPT_DIR/lib/network.sh"
# shellcheck source=lib/resources.sh
source "$SCRIPT_DIR/lib/resources.sh"
# shellcheck source=lib/common.sh
source "$SCRIPT_DIR/lib/common.sh"
# shellcheck source=cloud-init/common.sh
source "$SCRIPT_DIR/cloud-init/common.sh"
# shellcheck source=cloud-init/ubuntu.sh
source "$SCRIPT_DIR/cloud-init/ubuntu.sh"
# shellcheck source=cloud-init/alpine.sh
source "$SCRIPT_DIR/cloud-init/alpine.sh"
# shellcheck source=lib/platform.sh
source "$SCRIPT_DIR/lib/platform.sh"

get_libvirt_qemu_group() {
    if [[ -n "$LIBVIRT_QEMU_GROUP" ]] && getent group "$LIBVIRT_QEMU_GROUP" >/dev/null; then
        echo "$LIBVIRT_QEMU_GROUP"
        return 0
    fi

    if id -gn libvirt-qemu >/dev/null 2>&1; then
        id -gn libvirt-qemu
        return 0
    fi

    if getent group kvm >/dev/null; then
        echo "kvm"
        return 0
    fi

    if getent group qemu >/dev/null; then
        echo "qemu"
        return 0
    fi

    return 1
}

guest_vsock_host_cid() {
    local cid="${AGENTIC_GRPC_VSOCK_HOST_CID:-${AGENT_GRPC_VSOCK_HOST_CID:-$DEFAULT_VSOCK_HOST_CID}}"
    if [[ ! "$cid" =~ ^[0-9]+$ || "$cid" -eq 0 ]]; then
        log_error "Invalid guest VSock host CID: $cid"
        return 1
    fi
    echo "$cid"
}

grant_libvirt_storage_access() {
    local vm_dir="$1"
    local cloud_init_dir="$2"
    shift 2

    local qemu_group
    if ! qemu_group="$(get_libvirt_qemu_group)"; then
        log_error "Could not determine libvirt qemu group; set LIBVIRT_QEMU_GROUP"
        exit 1
    fi

    sudo chgrp "$qemu_group" "$vm_dir" "$cloud_init_dir"
    sudo chmod 750 "$vm_dir" "$cloud_init_dir"

    local path
    for path in "$@"; do
        if [[ -e "$path" ]]; then
            sudo chgrp "$qemu_group" "$path"
            sudo chmod 640 "$path"
        fi
    done
}

file_sha256() {
    sha256sum "$1" | awk '{print $1}'
}

usage() {
    cat <<EOF
Usage: $0 [OPTIONS] NAME

Rapidly provision an agent VM from a base image.

Arguments:
  NAME                  VM name (e.g., agent-01, claude-worker-1)

Options:
  -b, --base IMAGE      Base image (default: $DEFAULT_BASE)
                        Supports: ubuntu-22.04, ubuntu-24.04, ubuntu-25.10
  -p, --profile NAME    Provisioning profile (default: basic)
                        Profiles: basic, agentic-dev
  -l, --loadout FILE    Loadout manifest YAML (alternative to --profile)
                        See: images/qemu/loadouts/profiles/
  -c, --cpus NUM        CPU cores (default: $DEFAULT_CPUS)
  -m, --memory SIZE     Memory with unit (default: $DEFAULT_MEMORY)
  -d, --disk SIZE       Disk size (default: $DEFAULT_DISK)
  -k, --ssh-key FILE    SSH public key file (default: auto-detect)
  -s, --start           Start VM immediately after creation
  --autostart           Enable VM autostart with host (libvirt autostart)
  -w, --wait            Wait for VM to be SSH-ready (implies --start)
  --wait-ready          Wait for profile setup to complete (implies --wait)
  --ip IP               Static IP (default: DHCP)
  --network NET         libvirt network (default: default)
  --storage DIR         VM storage directory
  --agentshare          Enable agentshare mounts (global RO, inbox RW)
  --task-id ID          Task ID for task-specific mounts (implies --agentshare)
  --network-mode MODE   Egress control: isolated|allowlist|full (default: full)
                        isolated:  Management server only, no internet
                        allowlist: DNS-filtered + HTTPS only (requires Blocky)
                        full:      Unrestricted egress (legacy, default)
  --management HOST     Management server address (default: $MANAGEMENT_SERVER)
  --instance-id UUID    Canonical instance UUIDv7 for v2 routing (#252).
                        Written to /etc/agentic-sandbox/agent.env as
                        AGENT_INSTANCE_ID; agent-rs echoes it back on
                        registration so the management server can attach
                        the connection to the matching InstanceContext.

Resource Limits (libvirt tuning + cgroup v2):
  --mem-limit SIZE      Memory hard limit (default: 94% of --memory)
  --cpu-quota PERCENT   CPU quota percentage (default: cpus * 100)
  --io-weight NUM       I/O weight 100-1000 (default: 500)
  --io-read-limit SIZE  Read bandwidth limit (default: 500M)
  --io-write-limit SIZE Write bandwidth limit (default: 200M)
  --disk-quota SIZE     Inbox disk quota (default: 50G, requires XFS with prjquota)

  -n, --dry-run         Show what would be done
  -h, --help            Show this help

Security:
  Secure transport provisioning is required for new VMs. Legacy AGENT_SECRET
  bearer provisioning was retired in #412; agent.env is populated with UDS,
  vsock, mTLS, or bootstrap enrollment material instead.

Profiles:
  basic        Minimal setup with dev/break-glass SSH access
  agentic-dev  Node.js LTS, aiwg, Claude Code, dev tools.
               Direct runtime SSH keys are omitted by default; set
               AGENTIC_ENABLE_DIRECT_RUNTIME_SSH=1 only for explicit
               dev/break-glass access.

Resource Guidelines (for concurrent VMs):
  Single VM:    --cpus 8 --memory 16G
  2 concurrent: --cpus 4 --memory 8G  (default)
  4 concurrent: --cpus 2 --memory 4G

Examples:
  $0 agent-01                          # Quick start with defaults
  $0 --profile agentic-dev agent-01    # Full dev environment
  $0 --loadout profiles/claude-only.yaml agent-01  # Loadout manifest
  $0 --loadout profiles/dual-review.yaml agent-01  # Multi-provider setup
  $0 --start --wait agent-01           # Start and wait for SSH
  $0 --profile agentic-dev --wait-ready agent-01  # Wait for full setup
  $0 --cpus 2 --memory 4G agent-02     # Smaller VM for concurrency

EOF
}


_libvirt_os_xml() {
    local disk_path="$1"
    local firmware="${AGENTIC_VM_FIRMWARE:-uefi}"

    if [[ "$firmware" == "bios" ]]; then
        cat <<'EOF'
  <os>
    <type arch='x86_64' machine='q35'>hvm</type>
    <boot dev='hd'/>
  </os>
EOF
        return 0
    fi

    local code="${AGENTIC_OVMF_CODE:-}"
    local vars="${AGENTIC_OVMF_VARS:-}"
    if [[ -z "$code" ]]; then
        for candidate in \
            /usr/share/OVMF/OVMF_CODE_4M.fd \
            /usr/share/OVMF/OVMF_CODE.fd \
            /usr/share/edk2/ovmf/OVMF_CODE.fd; do
            if [[ -r "$candidate" ]]; then
                code="$candidate"
                break
            fi
        done
    fi
    if [[ -z "$vars" ]]; then
        for candidate in \
            /usr/share/OVMF/OVMF_VARS_4M.fd \
            /usr/share/OVMF/OVMF_VARS.fd \
            /usr/share/edk2/ovmf/OVMF_VARS.fd; do
            if [[ -r "$candidate" ]]; then
                vars="$candidate"
                break
            fi
        done
    fi
    if [[ -z "$code" || -z "$vars" ]]; then
        echo "ERROR: UEFI firmware requested but OVMF code/vars files were not found" >&2
        echo "       Set AGENTIC_VM_FIRMWARE=bios for BIOS images or AGENTIC_OVMF_CODE/AGENTIC_OVMF_VARS explicitly." >&2
        return 1
    fi

    local nvram_path="${disk_path%.qcow2}_VARS.fd"
    cat <<EOF
  <os>
    <type arch='x86_64' machine='q35'>hvm</type>
    <loader readonly='yes' type='pflash'>$code</loader>
    <nvram template='$vars'>$nvram_path</nvram>
    <boot dev='hd'/>
  </os>
EOF
}

# Define VM with virsh
define_vm() {
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
    local vsock_cid="${11:-}"

    # Resource limit parameters
    local mem_limit_mb="${12:-$memory_mb}"
    local cpu_quota_pct="${13:-$((cpus * 100))}"
    local io_weight="${14:-500}"
    local io_read_bps="${15:-524288000}"
    local io_write_bps="${16:-209715200}"

    # GPU passthrough config (optional, generated by loadout system)
    local gpu_config_path="${17:-}"

    # Optional per-VM vsock CID for ADR-023 transport wiring
    local vsock_element=""
    if [[ -n "$vsock_cid" ]]; then
        vsock_element="
    <vsock model='virtio'>
      <cid auto='no' address='$vsock_cid'/>
    </vsock>"
    fi

    # Generate libvirt XML
    local xml_path="${disk_path%.qcow2}.xml"

    # MAC address element (empty if not specified, lets libvirt generate one)
    local mac_element=""
    if [[ -n "$mac_address" ]]; then
        mac_element="<mac address='$mac_address'/>"
    fi

    # Virtiofs filesystems for agentshare
    local virtiofs_elements=""
    local memoryBacking=""
    if [[ "$use_agentshare" == "true" && -n "$inbox_path" ]]; then
        # Memory backing required for virtiofs
        memoryBacking="
  <memoryBacking>
    <source type='memfd'/>
    <access mode='shared'/>
  </memoryBacking>"

        # Note: virtiofs does not support <readonly/> in libvirt XML.
        # Read-only is enforced via mount options inside the VM (cloud-init).
        virtiofs_elements="
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs'/>
      <source dir='$AGENTSHARE_ROOT/global-ro'/>
      <target dir='agentglobal'/>
    </filesystem>
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs'/>
      <source dir='$inbox_path'/>
      <target dir='agentinbox'/>
    </filesystem>"

        # Add outbox mount if path provided (for task orchestration)
        if [[ -n "$outbox_path" ]]; then
            virtiofs_elements+="
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs'/>
      <source dir='$outbox_path'/>
      <target dir='agentoutbox'/>
    </filesystem>"
        fi
    fi

    # GPU passthrough hostdev element
    local gpu_hostdev=""
    if [[ -n "$gpu_config_path" && -f "$gpu_config_path" ]]; then
        # shellcheck source=/dev/null
        source "$gpu_config_path"
        if [[ "${GPU_ENABLED:-false}" == "true" && -n "${GPU_PCI_DEVICE:-}" ]]; then
            local pci_domain pci_bus pci_slot pci_function
            pci_domain="0x${GPU_PCI_DEVICE%%:*}"
            local rest="${GPU_PCI_DEVICE#*:}"
            pci_bus="0x${rest%%:*}"
            rest="${rest#*:}"
            pci_slot="0x${rest%%.*}"
            pci_function="0x${rest##*.}"
            gpu_hostdev="
    <hostdev mode='subsystem' type='pci' managed='yes'>
      <source>
        <address domain='$pci_domain' bus='$pci_bus' slot='$pci_slot' function='$pci_function'/>
      </source>
    </hostdev>"
        fi
    fi

    cat > "$xml_path" <<EOF
<domain type='kvm'>
  <name>$vm_name</name>
  <memory unit='MiB'>$memory_mb</memory>
  <vcpu placement='static'>$cpus</vcpu>$memoryBacking
  <memtune>
    <hard_limit unit='MiB'>$((mem_limit_mb + 256))</hard_limit>
    <soft_limit unit='MiB'>$mem_limit_mb</soft_limit>
  </memtune>
  <cputune>
    <shares>$((cpus * 1024))</shares>
    <period>100000</period>
    <quota>$((cpu_quota_pct * 1000))</quota>
  </cputune>
  <blkiotune>
    <weight>$io_weight</weight>
  </blkiotune>
$(_libvirt_os_xml "$disk_path")
  <features>
    <acpi/>
    <apic/>
  </features>
  <cpu mode='host-passthrough'/>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type='file' device='disk'>
      <driver name='qemu' type='qcow2' cache='writeback'/>
      <source file='$disk_path'/>
      <target dev='vda' bus='virtio'/>
    </disk>
    <disk type='file' device='cdrom'>
      <driver name='qemu' type='raw'/>
      <source file='$cloud_init_iso'/>
      <target dev='sda' bus='sata'/>
      <readonly/>
    </disk>
    <interface type='network'>
      <source network='$network'/>
      $mac_element
      <model type='virtio'/>
    </interface>$virtiofs_elements$gpu_hostdev$vsock_element
    <channel type='unix'>
      <target type='virtio' name='org.qemu.guest_agent.0'/>
    </channel>
    <serial type='pty'>
      <target port='0'/>
    </serial>
    <console type='pty'>
      <target type='serial' port='0'/>
    </console>
    <graphics type='vnc' port='-1' autoport='yes'/>
  </devices>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>restart</on_reboot>
  <on_crash>restart</on_crash>
</domain>
EOF

    virsh define "$xml_path" > /dev/null
    echo "$xml_path"
}

# Get VM IP address
get_vm_ip() {
    local vm_name="$1"
    local timeout="${2:-60}"
    local start_time=$(date +%s)

    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            return 1
        fi

        # Try virsh domifaddr
        local ip
        ip=$(virsh domifaddr "$vm_name" 2>/dev/null | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' | head -1)
        if [[ -n "$ip" ]]; then
            echo "$ip"
            return 0
        fi

        # Try qemu-guest-agent
        ip=$(virsh qemu-agent-command "$vm_name" '{"execute":"guest-network-get-interfaces"}' 2>/dev/null | \
             jq -r '.return[].["ip-addresses"][]? | select(.["ip-address-type"]=="ipv4") | .["ip-address"]' 2>/dev/null | \
             grep -v "^127\." | head -1)
        if [[ -n "$ip" ]]; then
            echo "$ip"
            return 0
        fi

        sleep 2
    done
}

# Wait for SSH to be ready
vm_ssh() {
    local ip="$1"
    local user="$2"
    local ssh_key="${3:-}"
    shift 3

    local ssh_opts=(-o ConnectTimeout=2 -o StrictHostKeyChecking=no -o BatchMode=yes)
    if [[ -n "$ssh_key" ]]; then
        ssh_opts+=(-o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes -i "$ssh_key")
    fi

    if [[ -n "$ssh_key" && ! -r "$ssh_key" ]]; then
        sudo -n ssh "${ssh_opts[@]}" "$user@$ip" "$@"
    else
        ssh "${ssh_opts[@]}" "$user@$ip" "$@"
    fi
}

# Wait for SSH to be ready
wait_for_ssh() {
    local ip="$1"
    local user="$2"
    local timeout="${3:-120}"
    local ssh_key="${4:-}"
    local start_time
    start_time=$(date +%s)
    local next_progress=0
    local progress_interval="${AGENTIC_VM_SSH_PROGRESS_SECONDS:-30}"

    while true; do
        local elapsed
        elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            return 1
        fi

        if [[ $elapsed -ge $next_progress ]]; then
            log_info "SSH wait progress for ${user}@${ip}: ${elapsed}/${timeout}s"
            next_progress=$((elapsed + progress_interval))
        fi

        if vm_ssh "$ip" "$user" "$ssh_key" "echo ready" 2>/dev/null | grep -q ready; then
            return 0
        fi

        sleep 2
    done
}

# Deploy agent-client binary and service to VM
deploy_agent_client() {
    local vm_name="$1"
    local ip="$2"

    local script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local repo_root="$(cd "$script_dir/../.." && pwd)"
    local deploy_script="$repo_root/scripts/provision-vm-agent.sh"

    if [[ ! -x "$deploy_script" ]]; then
        log_warn "Agent deploy script not found or not executable: $deploy_script"
        return 1
    fi

    local agent_binary="$repo_root/agent-rs/target/release/agent-client"
    if [[ ! -f "$agent_binary" ]]; then
        log_warn "Agent binary not found at $agent_binary"
        log_warn "Build with: cd agent-rs && cargo build --release"
        log_warn "Then run: ./scripts/provision-vm-agent.sh $vm_name"
        return 1
    fi

    log_info "Deploying agent-client service for $vm_name..."
    if "$deploy_script" "$vm_name" --ip "$ip" --server "$MANAGEMENT_SERVER" --force 2>&1; then
        log_success "Agent client deployed and started"
        return 0
    fi

    log_warn "Agent deploy failed; run manually: ./scripts/provision-vm-agent.sh $vm_name --ip $ip --server $MANAGEMENT_SERVER --force"
    return 1
}

# Wait for the deployed agent-client service and binary to match this checkout.
wait_for_agent_ready() {
    local ip="$1"
    local user="$2"
    local ssh_key="$3"
    local timeout="${4:-120}"
    local start_time=$(date +%s)
    local script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local repo_root="$(cd "$script_dir/../.." && pwd)"
    local agent_binary="$repo_root/agent-rs/target/release/agent-client"
    local expected_hash=""

    if [[ -f "$agent_binary" ]]; then
        expected_hash=$(sha256sum "$agent_binary")
        expected_hash="${expected_hash%% *}"
    fi

    log_info "Waiting for agent-client readiness (up to ${timeout}s)..."
    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            log_warn "Agent readiness timeout - check: journalctl -u agent-client -n 50 --no-pager"
            return 1
        fi

        local remote_hash=""
        remote_hash=$(vm_ssh "$ip" "$user" "$ssh_key" "sha256sum $AGENT_CLIENT_BIN 2>/dev/null" 2>/dev/null || true)
        remote_hash="${remote_hash%% *}"
        if [[ -n "$expected_hash" && "$remote_hash" != "$expected_hash" ]]; then
            sleep 3
            continue
        fi

        if vm_ssh "$ip" "$user" "$ssh_key" "systemctl is-active --quiet agent-client" 2>/dev/null; then
            log_success "Agent client ready"
            return 0
        fi

        sleep 3
    done
}

# Wait for agentshare virtiofs mounts and home-directory conveniences.
wait_for_agentshare_ready() {
    local ip="$1"
    local user="$2"
    local ssh_key="$3"
    local timeout="${4:-120}"
    local start_time=$(date +%s)

    log_info "Waiting for agentshare mounts (up to ${timeout}s)..."
    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            log_warn "Agentshare readiness timeout - check virtiofs mounts and /etc/fstab on VM"
            return 1
        fi

        if vm_ssh "$ip" "$user" "$ssh_key" "mountpoint -q /mnt/global && mountpoint -q /mnt/inbox && mountpoint -q /mnt/outbox && test -L /home/agent/global && test -L /home/agent/inbox && test -L /home/agent/workspace && test -L /home/agent/outbox" 2>/dev/null; then
            log_success "Agentshare mounts ready"
            return 0
        fi

        sleep 3
    done
}

# Wait for agentic-dev profile setup to complete
wait_for_setup_complete() {
    local ip="$1"
    local user="$2"
    local timeout="${3:-300}"  # 5 minutes default
    local ssh_key="${4:-}"
    local start_time=$(date +%s)

    log_info "Waiting for profile setup to complete (up to ${timeout}s)..."

    while true; do
        local elapsed=$(($(date +%s) - start_time))
        if [[ $elapsed -ge $timeout ]]; then
            log_warn "Setup timeout after ${timeout}s - check /var/log/agentic-setup.log on VM"
            return 1
        fi

        local status
        status=$(vm_ssh "$ip" "$user" "$ssh_key" "/opt/agentic-setup/check-ready.sh" 2>/dev/null || echo "pending")

        if [[ "$status" == "ready" ]]; then
            return 0
        fi

        echo -n "."
        sleep 5
    done
}

env_truthy() {
    case "${1,,}" in
        1|true|yes|on) return 0 ;;
        *) return 1 ;;
    esac
}

configure_grpc_local_ca_provisioning() {
    local vm_name="$1"
    local instance_id="$2"

    if ! env_truthy "${AGENTIC_GRPC_LOCAL_CA:-false}"; then
        unset AGENT_GRPC_TLS_CA_HOST_PATH
        unset AGENT_GRPC_TLS_CERT_HOST_PATH
        unset AGENT_GRPC_TLS_KEY_HOST_PATH
        return 0
    fi

    if [[ -z "$instance_id" ]]; then
        log_error "AGENTIC_GRPC_LOCAL_CA=1 requires --instance-id for SPIFFE URI-SAN provisioning"
        exit 1
    fi

    local helper="${AGENTIC_GRPC_LOCAL_CA_HELPER:-$PROJECT_ROOT/management/target/release/grpc-local-ca}"
    if [[ ! -x "$helper" ]]; then
        log_error "gRPC local CA helper not found or not executable: $helper"
        echo "  Build it first: cargo build --manifest-path $PROJECT_ROOT/management/Cargo.toml --release --bin grpc-local-ca"
        exit 1
    fi

    local ca_dir="${AGENTIC_GRPC_LOCAL_CA_DIR:-$SECRETS_DIR/grpc-local-ca}"
    local trust_domain="${AGENTIC_GRPC_LOCAL_CA_TRUST_DOMAIN:-sandbox.agentic.local}"
    local leaf_dir="${AGENTIC_GRPC_LOCAL_CA_LEAF_DIR:-$SECRETS_DIR/grpc-mtls/$vm_name}"
    local cert_path="$leaf_dir/agent.pem"
    local key_path="$leaf_dir/agent-key.pem"
    local helper_args=(
        issue-agent
        --ca-dir "$ca_dir"
        --trust-domain "$trust_domain"
        --instance-id "$instance_id"
        --cert "$cert_path"
        --key "$key_path"
    )
    if [[ -n "${AGENTIC_GRPC_CA_AGENT_LEAF_TTL_SECS:-}" ]]; then
        helper_args+=(--ttl-secs "$AGENTIC_GRPC_CA_AGENT_LEAF_TTL_SECS")
    fi
    if [[ -n "${AGENTIC_GRPC_CA_RENEW_BEFORE_SECS:-}" ]]; then
        helper_args+=(--renew-before-secs "$AGENTIC_GRPC_CA_RENEW_BEFORE_SECS")
    fi

    log_info "Issuing gRPC local mTLS client credential for $vm_name..."
    sudo -n "$helper" "${helper_args[@]}" >/dev/null

    export AGENT_GRPC_TLS_CA_HOST_PATH="$ca_dir/grpc-local-root-ca.pem"
    export AGENT_GRPC_TLS_CERT_HOST_PATH="$cert_path"
    export AGENT_GRPC_TLS_KEY_HOST_PATH="$key_path"
    export AGENT_GRPC_TLS_SERVER_NAME="${AGENT_GRPC_TLS_SERVER_NAME:-host.internal}"
    log_success "gRPC local mTLS client credential issued"
}

# Main provisioning function
provision_vm() {
    local vm_name="$1"
    local base="$2"
    local cpus="$3"
    local memory="$4"
    local disk="$5"
    local ssh_key_file="$6"
    local start_vm="$7"
    local wait_ssh="$8"
    local static_ip="$9"
    local network="${10}"
    local dry_run="${11}"
    local profile="${12:-}"
    local wait_ready="${13:-false}"
    local use_agentshare="${14:-false}"
    local task_id="${15:-}"
    local network_mode="${16:-full}"

    # Resource limit parameters
    local mem_limit="${17:-}"
    local cpu_quota="${18:-}"
    local io_weight="${19:-500}"
    local io_read_limit="${20:-}"
    local io_write_limit="${21:-}"
    local disk_quota="${22:-50G}"
    local loadout="${23:-}"

    # Resolve paths
    local base_image
    base_image=$(resolve_base_image "$base") || exit 1

    local ssh_key
    ssh_key=$(find_ssh_key "$ssh_key_file") || exit 1

    local memory_mb
    memory_mb=$(parse_memory_mb "$memory")

    # Generate deterministic MAC and allocate IP
    local mac_address
    mac_address=$(generate_mac_from_name "$vm_name")

    # Allocate static IP if not explicitly provided
    local allocated_ip="$static_ip"
    if [[ -z "$allocated_ip" ]]; then
        allocated_ip=$(allocate_ip_for_vm "$vm_name" "$network") || exit 1
    fi

    # Allocate stable VSock CID for ADR-023 provisioning only when backend supports it.
    local allocated_cid=""
    local reclaim_cid_on_exit=0
    if backend_supports_vsock_cid; then
        allocated_cid=$(allocate_cid_for_vm "$vm_name" "$instance_id") || exit 1
        reclaim_cid_on_exit=1
        trap 'if [[ "${reclaim_cid_on_exit:-0}" == "1" ]]; then remove_cid_allocation "$vm_name"; fi' EXIT
    fi
    unset AGENT_GRPC_VSOCK_CID
    unset AGENT_GRPC_VSOCK_PORT
    local allocated_vsock_port="${AGENTIC_GRPC_VSOCK_PORT:-${AGENT_GRPC_VSOCK_PORT:-}}"
    if [[ -n "$allocated_cid" && -n "$allocated_vsock_port" ]]; then
        local guest_vsock_cid
        guest_vsock_cid=$(guest_vsock_host_cid) || exit 1
        export AGENT_GRPC_VSOCK_CID="$guest_vsock_cid"
        export AGENT_GRPC_VSOCK_PORT="$allocated_vsock_port"
        log_info "Guest VSock transport target: host CID $guest_vsock_cid port $allocated_vsock_port (VM peer CID $allocated_cid)"
    elif [[ -n "$allocated_cid" ]]; then
        log_warn "Allocated vsock CID $allocated_cid but AGENTIC_GRPC_VSOCK_PORT is unset; vsock transport not emitted"
    fi

    # VM storage paths
    local vm_dir="$VM_STORAGE_DIR/$vm_name"
    local disk_path="$vm_dir/$vm_name.qcow2"
    local cloud_init_dir="$vm_dir/cloud-init"
    local cloud_init_iso="$vm_dir/cloud-init.iso"
    local base_image_sha256=""
    local base_image_manifest="$(dirname "$base_image")/manifest.json"
    local cloud_init_seed_sha256=""
    local loadout_manifest_path=""
    local loadout_source_sha256=""
    local loadout_resolved_sha256=""
    local gpu_config_path=""
    local carbonyl_sessions_enabled="false"
    local carbonyl_session_path=""
    local loadout_setup_wait_seconds="300"

    local profile_display
    if [[ -n "$loadout" ]]; then
        profile_display="loadout:$(basename "$loadout" .yaml)"
    else
        profile_display="${profile:-basic}"
    fi

    echo ""
    echo "╔═══════════════════════════════════════════════════════════════╗"
    echo "║     Provisioning Agent VM                                     ║"
    echo "╚═══════════════════════════════════════════════════════════════╝"
    echo ""
    echo "  VM Name:      $vm_name"
    echo "  Profile:      $profile_display"
    echo "  Base Image:   $(basename "$base_image")"
    echo "  Resources:    $cpus CPUs, ${memory_mb}MB RAM, $disk disk"
    echo "  SSH Key:      $ssh_key"
    echo "  Network:      $network"
    echo "  MAC Address:  $mac_address"
    echo "  IP Address:   $allocated_ip"
    if [[ -n "$allocated_cid" ]]; then
        echo "  VSock CID:    $allocated_cid"
    else
        echo "  VSock CID:    (backend "$ACTIVE_BACKEND" does not support host-side vsock CID wiring)"
    fi
    echo "  Storage:      $vm_dir"
    echo "  Management:   $MANAGEMENT_SERVER"
    echo "  Network Mode: $network_mode"
    if env_truthy "${AGENTIC_GRPC_LOCAL_CA:-false}"; then
        echo "  gRPC mTLS:    local CA provisioning enabled"
    fi
    if [[ "$use_agentshare" == "true" ]]; then
        if [[ -n "$task_id" ]]; then
            echo "  Agentshare:   Task mode (global RO, inbox RW, outbox RW)"
            echo "  Task ID:      $task_id"
        else
            echo "  Agentshare:   Enabled (global RO, inbox RW, outbox RW)"
        fi
    fi
    echo ""

    if [[ "$dry_run" == "true" ]]; then
        log_warn "DRY RUN - Would create VM with above settings"
        reclaim_cid_on_exit=0
        trap - EXIT
        return 0
    fi

    # Check if VM already exists
    if backend_vm_exists "$vm_name"; then
        log_error "VM '$vm_name' already exists"
        echo "  Use: virsh destroy $vm_name && virsh undefine $vm_name"
        exit 1
    fi

    # Create VM directory
    log_info "Creating VM storage directory..."
    sudo mkdir -p "$vm_dir"
    sudo mkdir -p "$cloud_init_dir"
    sudo chown -R "$(whoami):$(whoami)" "$vm_dir"
    # Keep the VM directory closed to other local users while granting the
    # libvirt qemu group enough access to open the disk and cloud-init ISO.
    grant_libvirt_storage_access "$vm_dir" "$cloud_init_dir"

    # Add DHCP reservation for static IP (non-fatal if it fails)
    log_info "Adding DHCP reservation ($mac_address → $allocated_ip)..."
    add_dhcp_reservation "$network" "$vm_name" "$mac_address" "$allocated_ip"

    # Create overlay disk (instant - uses backing file)
    log_info "Creating overlay disk from base image..."
    create_overlay_disk "$base_image" "$disk_path" "$disk"
    base_image_sha256=$(file_sha256 "$base_image")
    grant_libvirt_storage_access "$vm_dir" "$cloud_init_dir" "$disk_path"
    log_success "Overlay disk created: $disk_path"

    configure_grpc_local_ca_provisioning "$vm_name" "$instance_id"

    local secure_transport_provisioning="false"
    local secure_transport_status
    set +e
    secure_agent_transport_configured >/dev/null
    secure_transport_status=$?
    set -e
    if [[ "$secure_transport_status" -eq 0 ]]; then
        secure_transport_provisioning="true"
    elif [[ "$secure_transport_status" -ne 1 ]]; then
        exit "$secure_transport_status"
    fi

    if [[ "$secure_transport_provisioning" != "true" ]]; then
        log_error "Secure transport provisioning is required; legacy AGENT_SECRET provisioning was retired in #412"
        exit 1
    fi
    log_info "Secure transport/bootstrap provisioning configured; legacy agent secret omitted"
    local agent_secret=""

    # Generate ephemeral SSH key pair for automated access
    log_info "Generating ephemeral SSH key pair..."
    local ephemeral_ssh_pubkey
    ephemeral_ssh_pubkey=$(generate_agent_ssh_key "$vm_name")
    local ephemeral_ssh_key_path
    ephemeral_ssh_key_path=$(get_agent_ssh_key_path "$vm_name")
    log_success "Ephemeral SSH key generated: $ephemeral_ssh_key_path"

    # Generate health endpoint authentication token
    log_info "Generating health endpoint token..."
    local health_token
    health_token=$(generate_health_token "$vm_name")
    local health_token_hash
    health_token_hash=$(get_health_token_hash "$vm_name")
    log_success "Health token generated and hash stored"

    # Generate cloud-init (pass allocated IP for any network config)
    if [[ -n "$loadout" ]]; then
        # Loadout manifest path: resolve relative to loadouts/ directory
        local loadout_path="$loadout"
        local loadouts_dir="$SCRIPT_DIR/loadouts"
        if [[ ! "$loadout_path" = /* ]]; then
            loadout_path="$loadouts_dir/$loadout_path"
        fi
        # Resolve a bare catalog name (e.g. "full-suite" from the admin loadout
        # list / Cockpit UI) to its profile manifest. Full paths like
        # "profiles/basic.yaml" already resolve above; this only kicks in when
        # the literal path is absent and the loadout has no dir/extension.
        if [[ ! -f "$loadout_path" && "$loadout" != */* && "$loadout" != *.yaml ]]; then
            local profile_candidate="$loadouts_dir/profiles/$loadout.yaml"
            if [[ -f "$profile_candidate" ]]; then
                loadout_path="$profile_candidate"
            fi
        fi
        if [[ ! -f "$loadout_path" ]]; then
            log_error "Loadout manifest not found: $loadout_path"
            exit 1
        fi
        loadout_manifest_path="$loadout_path"
        loadout_source_sha256=$(file_sha256 "$loadout_path")

        log_info "Generating cloud-init from loadout: $(basename "$loadout_path")..."

        # Resolve manifest inheritance (output to temp file)
        local resolved_manifest
        resolved_manifest=$(mktemp /tmp/loadout-resolved.XXXXXX.yaml)
        "$loadouts_dir/resolve-manifest.sh" "$loadout_path" > "$resolved_manifest"
        loadout_resolved_sha256=$(file_sha256 "$resolved_manifest")

        # Override resources from manifest if CLI didn't set them explicitly
        local manifest_cpus manifest_memory manifest_disk manifest_setup_wait
        manifest_cpus=$(python3 -c "import yaml; d=yaml.safe_load(open('$resolved_manifest')); print(d.get('resources',{}).get('cpus',''))" 2>/dev/null || true)
        manifest_memory=$(python3 -c "import yaml; d=yaml.safe_load(open('$resolved_manifest')); print(d.get('resources',{}).get('memory',''))" 2>/dev/null || true)
        manifest_disk=$(python3 -c "import yaml; d=yaml.safe_load(open('$resolved_manifest')); print(d.get('resources',{}).get('disk',''))" 2>/dev/null || true)
        manifest_setup_wait=$(python3 -c "import yaml; d=yaml.safe_load(open('$resolved_manifest')); print(d.get('readiness',{}).get('setup_timeout_seconds',''))" 2>/dev/null || true)
        if [[ -n "$manifest_setup_wait" ]]; then
            if [[ ! "$manifest_setup_wait" =~ ^[0-9]+$ || "$manifest_setup_wait" -lt 1 ]]; then
                log_error "Invalid readiness.setup_timeout_seconds in loadout: $manifest_setup_wait"
                exit 1
            fi
            loadout_setup_wait_seconds="$manifest_setup_wait"
        fi
        if grep -q '/opt/carbonyl' "$resolved_manifest"; then
            carbonyl_sessions_enabled="true"
            carbonyl_session_path="$vm_dir/carbonyl-sessions"
            log_info "Carbonyl session persistence enabled: $carbonyl_session_path"
        fi

        # Apply manifest resources (CLI flags take precedence via already being set)
        [[ "$cpus" == "$DEFAULT_CPUS" && -n "$manifest_cpus" ]] && cpus="$manifest_cpus"
        [[ "$memory" == "$DEFAULT_MEMORY" && -n "$manifest_memory" ]] && memory="$manifest_memory"
        [[ "$disk" == "$DEFAULT_DISK" && -n "$manifest_disk" ]] && disk="$manifest_disk"

        # Recalculate memory_mb if changed
        memory_mb=$(parse_memory_mb "$memory")

        # Generate cloud-init from manifest. #252: export instance_id so
        # loadout-generated cloud-init scripts can also inject
        # AGENT_INSTANCE_ID via the env var contract.
        export AGENT_INSTANCE_ID="$instance_id"
        "$loadouts_dir/generate-from-manifest.sh" "$resolved_manifest" \
            "$vm_name" "$ssh_key" "$cloud_init_dir" \
            "$use_agentshare" "$agent_secret" "$ephemeral_ssh_pubkey" \
            "$mac_address" "$network_mode" "$health_token" "$MANAGEMENT_SERVER"
        rm -f "$resolved_manifest"

        # Check if GPU config was generated
        if [[ -f "$cloud_init_dir/gpu-config" ]]; then
            gpu_config_path="$cloud_init_dir/gpu-config"
            log_info "GPU passthrough enabled (config: $gpu_config_path)"
        fi
    else
        log_info "Generating cloud-init configuration (profile: $profile_display)..."
        local os_type
        os_type=$(detect_os_type "$base")
        # #252: export instance_id so cloud-init generators (which read
        # env vars rather than positional args) can inject
        # AGENT_INSTANCE_ID into /etc/agentic-sandbox/agent.env.
        export AGENT_INSTANCE_ID="$instance_id"
        if [[ "$os_type" == "alpine" ]]; then
            generate_alpine_cloud_init "$vm_name" "$ssh_key" "$allocated_ip" "$cloud_init_dir" "$profile" "$use_agentshare" "$agent_secret" "$ephemeral_ssh_pubkey" "$mac_address" "$network_mode" "$health_token"
        else
            generate_cloud_init "$vm_name" "$ssh_key" "$allocated_ip" "$cloud_init_dir" "$profile" "$use_agentshare" "$agent_secret" "$ephemeral_ssh_pubkey" "$mac_address" "$network_mode" "$health_token"
        fi
    fi
    if [[ "$carbonyl_sessions_enabled" == "true" ]]; then
        log_info "Creating carbonyl session storage: $carbonyl_session_path"
        sudo mkdir -p "$carbonyl_session_path"
        # Loadout cloud-init creates agent as the primary user. Preserve host
        # privacy with 0700 while matching the default guest uid/gid mapping.
        sudo chown 1000:1000 "$carbonyl_session_path"
        sudo chmod 700 "$carbonyl_session_path"
        log_success "Carbonyl session storage created"
    fi

    create_cloud_init_iso "$cloud_init_dir" "$cloud_init_iso"
    cloud_init_seed_sha256=$(file_sha256 "$cloud_init_iso")
    # The ISO must be readable by libvirt qemu so the VM can boot.
    grant_libvirt_storage_access "$vm_dir" "$cloud_init_dir" "$disk_path" "$cloud_init_iso"
    sudo find "$cloud_init_dir" -type f -exec chmod 600 {} \; 2>/dev/null || \
        find "$cloud_init_dir" -type f -exec chmod 600 {} \; 2>/dev/null || true
    sudo rm -rf "$cloud_init_dir" 2>/dev/null || rm -rf "$cloud_init_dir"
    log_success "Cloud-init ISO created"

    # Create agentshare inbox/outbox if enabled
    local inbox_path=""
    local outbox_path=""
    if [[ "$use_agentshare" == "true" ]]; then
        if [[ ! -d "$AGENTSHARE_ROOT/global" ]]; then
            log_error "Agentshare not initialized. Run: sudo ./setup-agentshare.sh"
            exit 1
        fi

        if [[ -n "$task_id" ]]; then
            # Task orchestration mode: use task-specific directories
            inbox_path="$TASKS_ROOT/${task_id}/inbox"
            outbox_path="$TASKS_ROOT/${task_id}/outbox"
            log_info "Creating task storage for task $task_id"
            sudo mkdir -p "$inbox_path"
            sudo mkdir -p "$outbox_path"/{progress,artifacts}
            sudo chmod 755 "$TASKS_ROOT/${task_id}"
            sudo chmod 755 "$inbox_path"
            sudo chmod 755 "$outbox_path" "$outbox_path"/{progress,artifacts}
            # Initialize progress files
            sudo touch "$outbox_path/progress/stdout.log"
            sudo touch "$outbox_path/progress/stderr.log"
            sudo touch "$outbox_path/progress/events.jsonl"
            sudo chmod 666 "$outbox_path/progress/"*.log "$outbox_path/progress/"*.jsonl
            log_success "Task storage created"
            # Setup disk quota for task inbox
            setup_inbox_quota "$inbox_path" "$disk_quota" "task_${task_id:0:8}"
        else
            # Legacy agent mode: per-VM inbox with outbox
            inbox_path="$AGENTSHARE_ROOT/${vm_name}-inbox"
            outbox_path="$AGENTSHARE_ROOT/${vm_name}-outbox"
            log_info "Creating agentshare inbox: $inbox_path"
            sudo mkdir -p "$inbox_path"/{outputs,logs,runs}
            sudo mkdir -p "$outbox_path"/{progress,artifacts}
            sudo chmod 777 "$inbox_path"
            sudo chmod 755 "$inbox_path"/{outputs,logs,runs}
            sudo chmod 777 "$outbox_path"
            sudo chmod 755 "$outbox_path"/{progress,artifacts}
            log_success "Agentshare inbox/outbox created"
            # Setup disk quota for VM inbox
            setup_inbox_quota "$inbox_path" "$disk_quota" "agent_${vm_name}"
        fi
    fi

    # Calculate resource limits
    local limits mem_limit_mb cpu_quota_pct io_read_bps io_write_bps
    limits=$(calculate_resource_limits "$cpus" "$memory_mb" "$mem_limit" "$cpu_quota" "$io_read_limit" "$io_write_limit")
    read -r mem_limit_mb cpu_quota_pct io_read_bps io_write_bps <<< "$limits"
    log_info "Resource limits: mem=${mem_limit_mb}M cpu=${cpu_quota_pct}% io=${io_weight}w/${io_read_bps}r/${io_write_bps}w"

    # Define VM via active backend
    log_info "Defining VM (backend: $ACTIVE_BACKEND)..."
    local xml_path
    local -a backend_create_args=(
        "$vm_name"
        "$disk_path"
        "$cloud_init_iso"
        "$cpus"
        "$memory_mb"
        "$network"
        "$mac_address"
        "$use_agentshare"
        "$inbox_path"
        "$outbox_path"
        "$mem_limit_mb"
        "$cpu_quota_pct"
        "$io_weight"
        "$io_read_bps"
        "$io_write_bps"
        "$gpu_config_path"
        "$carbonyl_session_path"
    )

    if backend_supports_vsock_cid; then
        backend_create_args+=("$allocated_cid")
    fi

    xml_path=$(backend_create_vm "${backend_create_args[@]}")
    log_success "VM defined: $vm_name"

    # Enable autostart if requested
    if [[ "$autostart" == "true" ]]; then
        backend_set_autostart "$vm_name" "true"
        log_info "Autostart enabled for $vm_name"
    fi

    local setup_wait_timeout="${AGENTIC_VM_SETUP_WAIT_SECONDS:-${LOADOUT_SETUP_WAIT_SECONDS:-$loadout_setup_wait_seconds}}"
    if [[ ! "$setup_wait_timeout" =~ ^[0-9]+$ || "$setup_wait_timeout" -lt 1 ]]; then
        log_error "Invalid setup wait timeout: $setup_wait_timeout"
        exit 1
    fi

    write_vm_info() {
        local provisioning_status="$1"
        local carbonyl_json=""
        if [[ -n "$carbonyl_session_path" ]]; then
            carbonyl_json=",
    \"carbonyl_sessions\": {
        \"host_path\": \"$carbonyl_session_path\",
        \"guest_path\": \"/home/agent/.local/share/carbonyl-agent/sessions\",
        \"mode\": \"0700\"
    }"
        fi

        local agentshare_json=""
        if [[ "$use_agentshare" == "true" ]]; then
            local task_json=""
            if [[ -n "$task_id" ]]; then
                task_json=",
        \"task_id\": \"$task_id\""
            fi
            agentshare_json=",
    \"agentshare\": {
        \"enabled\": true,
        \"global\": \"$AGENTSHARE_ROOT/global\",
        \"inbox\": \"$inbox_path\",
        \"outbox\": \"$outbox_path\"$task_json
    }"
        fi

        local loadout_json="null"
        if [[ -n "$loadout_manifest_path" ]]; then
            loadout_json="{
            \"path\": \"$loadout_manifest_path\",
            \"source_sha256\": \"$loadout_source_sha256\",
            \"resolved_sha256\": \"$loadout_resolved_sha256\"
        }"
        fi

        cat > "$vm_dir/vm-info.json" <<EOF
{
    "name": "$vm_name",
    "instance_id": "$instance_id",
    "ip": "$allocated_ip",
    "vsock_cid": "$allocated_cid",
    "mac": "$mac_address",
    "profile": "$profile_display",
    "base_image": "$(basename "$base_image")",
    "provenance": {
        "base_image": {
            "path": "$base_image",
            "sha256": "$base_image_sha256",
            "manifest": "$base_image_manifest"
        },
        "cloud_init_seed": {
            "path": "$cloud_init_iso",
            "sha256": "$cloud_init_seed_sha256",
            "mode": "0640",
            "source_dir_retained": false
        },
        "loadout": $loadout_json
    },
    "created": "$(date -Iseconds)",
    "provisioning": {
        "status": "$provisioning_status",
        "wait_ready": $wait_ready,
        "setup_timeout_seconds": $setup_wait_timeout
    },
    "management": {
        "server": "$MANAGEMENT_SERVER",
        "agent_id": "$vm_name",
        "secret_hash": null,
        "ssh_key_path": "$ephemeral_ssh_key_path"
    }$agentshare_json$carbonyl_json
}
EOF
        # vm-info.json includes the ephemeral SSH key path; keep it owner-only.
        chmod 600 "$vm_dir/vm-info.json" 2>/dev/null || sudo chmod 600 "$vm_dir/vm-info.json"
    }

    # Persist diagnostic metadata before any wait gate can exit nonzero.
    write_vm_info "defined"

    # Start if requested
    if [[ "$start_vm" == "true" ]]; then
        log_info "Starting VM..."
        backend_start_vm "$vm_name"
        log_success "VM started"
        write_vm_info "running"

        # IP is already known (pre-assigned via DHCP reservation)
        log_info "VM will be available at $allocated_ip"

        # Wait for SSH if requested
        if [[ "$wait_ssh" == "true" ]]; then
            log_info "Waiting for SSH to be ready at $allocated_ip..."
            local ssh_wait_timeout="${AGENTIC_VM_SSH_WAIT_SECONDS:-${SSH_WAIT_SECONDS:-300}}"
            if wait_for_ssh "$allocated_ip" "$SERVICE_USER" "$ssh_wait_timeout" "$ephemeral_ssh_key_path"; then
                log_success "SSH ready!"

                # Deploy agent-client binary and service. In --wait-ready mode,
                # deployment failure is terminal because readiness would be false.
                if ! deploy_agent_client "$vm_name" "$allocated_ip"; then
                    if [[ "$wait_ready" == "true" ]]; then
                        write_vm_info "agent_client_deploy_failed"
                        exit 1
                    fi
                fi

                if [[ "$wait_ready" == "true" ]]; then
                    echo ""
                    if [[ "$use_agentshare" == "true" ]]; then
                        if ! wait_for_agentshare_ready "$allocated_ip" "$SERVICE_USER" "$ephemeral_ssh_key_path" 180; then
                            write_vm_info "timeout_waiting_for_agentshare"
                            exit 1
                        fi
                    fi
                    if ! wait_for_agent_ready "$allocated_ip" "$SERVICE_USER" "$ephemeral_ssh_key_path" 180; then
                        write_vm_info "timeout_waiting_for_agent"
                        exit 1
                    fi

                    # Some loadouts install an additional readiness script for profile bootstrap.
                    # Basic SSH-only profiles do not, so skip this gate unless the VM exposes it.
                    if vm_ssh "$allocated_ip" "$SERVICE_USER" "$ephemeral_ssh_key_path" "test -x /opt/agentic-setup/check-ready.sh" 2>/dev/null; then
                        if wait_for_setup_complete "$allocated_ip" "$SERVICE_USER" "$setup_wait_timeout" "$ephemeral_ssh_key_path"; then
                            echo ""
                            log_success "Profile setup complete!"
                        else
                            write_vm_info "timeout_waiting_for_setup"
                            exit 1
                        fi
                    else
                        log_info "No profile setup readiness script present; skipping profile setup wait"
                    fi
                fi
            else
                log_warn "SSH not responding (cloud-init may still be running)"
                if [[ "$wait_ready" == "true" ]]; then
                    write_vm_info "timeout_waiting_for_ssh"
                    exit 1
                fi
            fi
        fi
    fi

    # Save final VM status to config file.
    local final_provisioning_status="defined"
    if [[ "$start_vm" == "true" ]]; then
        if [[ "$wait_ready" == "true" ]]; then
            final_provisioning_status="ready"
        else
            final_provisioning_status="running"
        fi
    fi
    write_vm_info "$final_provisioning_status"

    # Summary
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    log_success "VM provisioned successfully!"
    reclaim_cid_on_exit=0
    trap - EXIT
    echo "═══════════════════════════════════════════════════════════════"
    echo ""
    echo "  VM Name:    $vm_name"
    echo "  IP:         $allocated_ip  (pre-assigned)"
    echo "  MAC:        $mac_address"
    echo "  Storage:    $vm_dir"
    if [[ "$use_agentshare" == "true" ]]; then
        echo ""
        if [[ -n "$task_id" ]]; then
            echo "  Task Storage:"
            echo "    Task ID:  $task_id"
            echo "    Global:   $AGENTSHARE_ROOT/global  (VM: /mnt/global, ~/global)"
            echo "    Inbox:    $inbox_path  (VM: /mnt/inbox, ~/inbox, ~/workspace)"
            echo "    Outbox:   $outbox_path  (VM: /mnt/outbox, ~/outbox)"
        else
            echo "  Agentshare:"
            echo "    Global:   $AGENTSHARE_ROOT/global  (VM: /mnt/global, ~/global)"
            echo "    Inbox:    $inbox_path  (VM: /mnt/inbox, ~/inbox, ~/workspace)"
            echo "    Outbox:   $outbox_path  (VM: /mnt/outbox, ~/outbox)"
        fi
    fi
    if [[ -n "$carbonyl_session_path" ]]; then
        echo "  Carbonyl sessions: $carbonyl_session_path  (VM: /home/agent/.local/share/carbonyl-agent/sessions)"
    fi
    if [[ "$start_vm" == "true" ]]; then
        echo "  Status:     Running"
        echo ""
        echo "  Connect:    ssh $SERVICE_USER@$allocated_ip"
        echo "  Console:    virsh console $vm_name"
    else
        echo "  Status:     Defined (not started)"
        echo ""
        echo "  Start:      virsh start $vm_name"
        echo "  Connect:    ssh $SERVICE_USER@$allocated_ip  (after start)"
    fi
    echo ""
    echo "  Management:"
    echo "    Stop:     virsh shutdown $vm_name"
    echo "    Force:    virsh destroy $vm_name"
    echo "    Delete:   virsh undefine $vm_name && rm -rf $vm_dir"
    echo ""
}

# Main
main() {
    local vm_name=""
    local base="$DEFAULT_BASE"
    local cpus="$DEFAULT_CPUS"
    local memory="$DEFAULT_MEMORY"
    local disk="$DEFAULT_DISK"
    local ssh_key_file=""
    local start_vm=false
    local wait_ssh=false
    local wait_ready=false
    local autostart=false
    local static_ip=""
    local network="default"
    local dry_run=false
    local profile=""
    local loadout=""
    local use_agentshare=false
    local task_id=""
    local network_mode="$DEFAULT_NETWORK_MODE"
    local instance_id=""

    # Resource limit options (empty = auto-calculate defaults)
    local mem_limit=""
    local cpu_quota=""
    local io_weight="500"
    local io_read_limit=""
    local io_write_limit=""
    local disk_quota="50G"

    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -b|--base)
                base="$2"
                shift 2
                ;;
            -p|--profile)
                profile="$2"
                shift 2
                ;;
            -l|--loadout)
                loadout="$2"
                shift 2
                ;;
            -c|--cpus)
                cpus="$2"
                shift 2
                ;;
            -m|--memory)
                memory="$2"
                shift 2
                ;;
            -d|--disk)
                disk="$2"
                shift 2
                ;;
            -k|--ssh-key)
                ssh_key_file="$2"
                shift 2
                ;;
            -s|--start)
                start_vm=true
                shift
                ;;
            --autostart)
                autostart=true
                shift
                ;;
            -w|--wait)
                start_vm=true
                wait_ssh=true
                shift
                ;;
            --wait-ready)
                start_vm=true
                wait_ssh=true
                wait_ready=true
                shift
                ;;
            --ip)
                static_ip="$2"
                shift 2
                ;;
            --network)
                network="$2"
                shift 2
                ;;
            --storage)
                VM_STORAGE_DIR="$2"
                shift 2
                ;;
            --agentshare)
                use_agentshare=true
                shift
                ;;
            --task-id)
                task_id="$2"
                use_agentshare=true  # task-id implies agentshare
                shift 2
                ;;
            --network-mode)
                network_mode="$2"
                if [[ ! "$network_mode" =~ ^(isolated|allowlist|full)$ ]]; then
                    log_error "Invalid network mode: $network_mode (must be isolated|allowlist|full)"
                    exit 1
                fi
                shift 2
                ;;
            --management)
                MANAGEMENT_SERVER="$2"
                shift 2
                ;;
            --instance-id)
                instance_id="$2"
                shift 2
                ;;
            --mem-limit)
                mem_limit="$2"
                shift 2
                ;;
            --cpu-quota)
                cpu_quota="$2"
                shift 2
                ;;
            --io-weight)
                io_weight="$2"
                shift 2
                ;;
            --io-read-limit)
                io_read_limit="$2"
                shift 2
                ;;
            --io-write-limit)
                io_write_limit="$2"
                shift 2
                ;;
            --disk-quota)
                disk_quota="$2"
                shift 2
                ;;
            -n|--dry-run)
                dry_run=true
                shift
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            -*)
                log_error "Unknown option: $1"
                usage
                exit 1
                ;;
            *)
                vm_name="$1"
                shift
                ;;
        esac
    done

    # Validate
    if [[ -z "$vm_name" ]]; then
        log_error "VM name is required"
        usage
        exit 1
    fi

    # --profile and --loadout are mutually exclusive
    if [[ -n "$profile" && -n "$loadout" ]]; then
        log_error "--profile and --loadout cannot be used together"
        exit 1
    fi

    # Ensure storage directory exists
    if [[ ! -d "$VM_STORAGE_DIR" ]]; then
        sudo mkdir -p "$VM_STORAGE_DIR"
        sudo chown "$(whoami):$(whoami)" "$VM_STORAGE_DIR"
    fi

    provision_vm "$vm_name" "$base" "$cpus" "$memory" "$disk" \
                 "$ssh_key_file" "$start_vm" "$wait_ssh" "$static_ip" \
                 "$network" "$dry_run" "$profile" "$wait_ready" "$use_agentshare" \
                 "$task_id" "$network_mode" \
                 "$mem_limit" "$cpu_quota" "$io_weight" "$io_read_limit" "$io_write_limit" \
                 "$disk_quota" "$loadout"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
