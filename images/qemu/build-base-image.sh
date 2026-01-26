#!/bin/bash
# build-base-image.sh - Build agent-ready VM base images for agentic-sandbox
#
# This script creates Ubuntu server base images pre-configured with:
# - qemu-guest-agent for virsh command execution
# - cloud-init for first-boot configuration
# - SSH server for fallback access
# - Agent user with sudo privileges

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISO_DIR="${ISO_DIR:-/mnt/ops/isos/linux}"
BASE_DIR="${BASE_DIR:-/mnt/ops/base-images}"
AUTOINSTALL_DIR="$SCRIPT_DIR/autoinstall"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}=>${NC} $*"; }
log_success() { echo -e "${GREEN}✓${NC} $*"; }
log_warn() { echo -e "${YELLOW}!${NC} $*"; }
log_error() { echo -e "${RED}✗${NC} $*" >&2; }

usage() {
    cat <<EOF
Usage: $0 [OPTIONS] VERSION

Build an agent-ready Ubuntu base image for agentic-sandbox QEMU adapter.

Arguments:
  VERSION    Ubuntu version: 22.04, 24.04, or 25.10

Options:
  -d, --disk-size SIZE    Disk size (default: 40G)
  -r, --ram SIZE          RAM for build VM in MB (default: 4096)
  -c, --cpus NUM          CPUs for build VM (default: 2)
  -o, --output PATH       Output image path (default: auto)
  -n, --dry-run           Show commands without executing
  -h, --help              Show this help

Environment:
  ISO_DIR     Directory containing Ubuntu ISOs (default: /mnt/ops/isos/linux)
  BASE_DIR    Directory for output images (default: /mnt/ops/base-images)

Examples:
  $0 24.04                     # Build Ubuntu 24.04 agent image
  $0 --disk-size 60G 24.04     # With 60GB disk
  $0 --dry-run 25.10           # Preview commands for 25.10

Output:
  Creates \${BASE_DIR}/ubuntu-server-\${VERSION}-agent.qcow2
EOF
}

check_dependencies() {
    local deps=("qemu-img" "virt-install" "genisoimage" "virt-customize")
    local missing=()

    for cmd in "${deps[@]}"; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done

    if [[ ${#missing[@]} -gt 0 ]]; then
        log_error "Missing dependencies: ${missing[*]}"
        echo "Install with: sudo apt install qemu-utils virtinst genisoimage libguestfs-tools"
        exit 1
    fi
}

resolve_iso_path() {
    local version="$1"
    local iso_path

    # Try exact version match first
    iso_path="$ISO_DIR/ubuntu-${version}-live-server-amd64.iso"
    [[ -f "$iso_path" ]] && echo "$iso_path" && return

    # Try with point release (e.g., 24.04.3)
    for iso in "$ISO_DIR"/ubuntu-${version}*-live-server-amd64.iso; do
        [[ -f "$iso" ]] && echo "$iso" && return
    done

    return 1
}

generate_autoinstall_iso() {
    local version="$1"
    local output="$2"

    log_info "Generating autoinstall ISO..."

    local tmpdir
    tmpdir=$(mktemp -d)

    # Copy templates or generate defaults
    if [[ -f "$AUTOINSTALL_DIR/user-data.template" ]]; then
        cp "$AUTOINSTALL_DIR/user-data.template" "$tmpdir/user-data"
    else
        cat > "$tmpdir/user-data" << 'USERDATA'
#cloud-config
autoinstall:
  version: 1
  locale: en_US.UTF-8
  keyboard:
    layout: us
  storage:
    layout:
      name: lvm
      sizing-policy: scaled
  identity:
    hostname: agent-base
    username: agent
    password: "$6$rounds=4096$saltsalt$K7E9eFjQpCzNrLj1WfFGPJzxp5Y.RZcxz9Y/2E0hbhVL2HxF5HgvGvNpP3KZJqYmYQ.e5FJYqXTGpOLgKWO6Y."
  ssh:
    install-server: true
    allow-pw: false
  packages:
    - qemu-guest-agent
    - python3
    - python3-pip
    - curl
    - wget
    - git
    - jq
    - htop
    - tmux
    - vim
    - ca-certificates
  late-commands:
    - curtin in-target -- systemctl enable qemu-guest-agent
    - curtin in-target -- passwd -d agent
    - curtin in-target -- apt-get clean
    - curtin in-target -- cloud-init clean --logs
USERDATA
    fi

    if [[ -f "$AUTOINSTALL_DIR/meta-data.template" ]]; then
        cp "$AUTOINSTALL_DIR/meta-data.template" "$tmpdir/meta-data"
    else
        echo "instance-id: agent-base" > "$tmpdir/meta-data"
    fi

    genisoimage -output "$output" -volid cidata -joliet -rock "$tmpdir" 2>/dev/null
    rm -rf "$tmpdir"
    log_success "Created autoinstall ISO: $output"
}

build_image() {
    local version="$1"
    local disk_size="$2"
    local ram="$3"
    local cpus="$4"
    local output="$5"
    local dry_run="$6"

    local iso_path
    if ! iso_path=$(resolve_iso_path "$version"); then
        log_error "ISO not found for Ubuntu $version"
        echo "Available ISOs in $ISO_DIR:"
        ls -la "$ISO_DIR"/*.iso 2>/dev/null || echo "  (none)"
        exit 1
    fi

    local image_name="ubuntu-server-${version}-agent.qcow2"
    local image_path="${output:-$BASE_DIR/$image_name}"
    local vm_name="build-agent-${version}"

    echo ""
    log_info "Building agent base image"
    echo "  Version:    Ubuntu $version"
    echo "  ISO:        $iso_path"
    echo "  Output:     $image_path"
    echo "  Disk:       $disk_size"
    echo "  RAM:        ${ram}MB"
    echo "  CPUs:       $cpus"
    echo ""

    if [[ "$dry_run" == "true" ]]; then
        log_warn "[DRY RUN] Would build: $image_name"
        return 0
    fi

    if [[ -f "$image_path" ]]; then
        log_warn "Image already exists: $image_path"
        read -p "Overwrite? (y/N) " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            log_info "Aborted"
            return 1
        fi
        sudo chattr -i "$image_path" 2>/dev/null || true
        rm -f "$image_path"
    fi

    mkdir -p "$(dirname "$image_path")"

    log_info "Creating disk image..."
    qemu-img create -f qcow2 "$image_path" "$disk_size"

    local autoinstall_iso="/tmp/autoinstall-${version}-$$.iso"
    generate_autoinstall_iso "$version" "$autoinstall_iso"

    virsh destroy "$vm_name" 2>/dev/null || true
    virsh undefine "$vm_name" --nvram 2>/dev/null || true

    log_info "Starting unattended installation (this takes 10-20 minutes)..."
    echo "    You can monitor progress with: virsh console $vm_name"
    echo ""

    virt-install \
        --name "$vm_name" \
        --ram "$ram" \
        --vcpus "$cpus" \
        --disk "path=$image_path,format=qcow2" \
        --cdrom "$iso_path" \
        --disk "path=$autoinstall_iso,device=cdrom" \
        --os-variant "ubuntu${version}" \
        --network network=default \
        --graphics none \
        --console pty,target_type=serial \
        --boot uefi \
        --extra-args "autoinstall console=ttyS0,115200n8" \
        --wait -1 \
        --noautoconsole

    log_info "Waiting for installation to complete..."
    while virsh domstate "$vm_name" 2>/dev/null | grep -q running; do
        sleep 10
    done

    log_info "Running post-install configuration..."
    virt-customize -a "$image_path" \
        --run-command 'systemctl enable qemu-guest-agent' \
        --run-command 'apt-get clean' \
        --run-command 'cloud-init clean --logs' \
        --run-command 'rm -rf /var/lib/apt/lists/*' 2>/dev/null || true

    virsh undefine "$vm_name" --nvram 2>/dev/null || true
    rm -f "$autoinstall_iso"

    log_info "Optimizing image size..."
    virt-sparsify --in-place "$image_path" 2>/dev/null || true

    log_info "Freezing image (read-only)..."
    chmod 444 "$image_path"
    sudo chattr +i "$image_path" 2>/dev/null || true

    echo ""
    log_success "Built: $image_path"
    echo "  Size: $(du -h "$image_path" | cut -f1)"
}

main() {
    local version=""
    local disk_size="40G"
    local ram="4096"
    local cpus="2"
    local output=""
    local dry_run="false"

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -d|--disk-size) disk_size="$2"; shift 2 ;;
            -r|--ram) ram="$2"; shift 2 ;;
            -c|--cpus) cpus="$2"; shift 2 ;;
            -o|--output) output="$2"; shift 2 ;;
            -n|--dry-run) dry_run="true"; shift ;;
            -h|--help) usage; exit 0 ;;
            22.04|24.04|25.10) version="$1"; shift ;;
            *) log_error "Unknown option: $1"; usage; exit 1 ;;
        esac
    done

    if [[ -z "$version" ]]; then
        log_error "VERSION required"
        usage
        exit 1
    fi

    check_dependencies
    build_image "$version" "$disk_size" "$ram" "$cpus" "$output" "$dry_run"
}

main "$@"
