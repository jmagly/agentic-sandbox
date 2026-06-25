#!/bin/bash
# build-base-image.sh - Build agent-ready VM base images for agentic-sandbox
#
# This script creates Ubuntu server base images pre-configured with:
# - qemu-guest-agent for virsh command execution
# - cloud-init for first-boot configuration
# - SSH server for fallback access
# - Agent user with sudo privileges
# - agent-client binary + self-enrolling agent-client.service (#561)
# - virtio-vsock guest transport module loaded on boot (ADR-023 vsock path)
# - refreshed kernel/libraries (apt full-upgrade) at bake time

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISO_DIR="${ISO_DIR:-/mnt/ops/isos/linux}"
BASE_DIR="${BASE_DIR:-/mnt/ops/base-images}"
AUTOINSTALL_DIR="$SCRIPT_DIR/autoinstall"

# #258: ISO + qcow2 integrity verification helpers
# shellcheck source=lib/verify.sh
source "$SCRIPT_DIR/lib/verify.sh"

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
  -y, --yes, --force      Overwrite an existing output image without prompting
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

VSOCK_GUEST_MODULE="vmw_vsock_virtio_transport"

ensure_output_directory_writable() {
    local image_path="$1"
    local dry_run="$2"
    local output_dir
    output_dir="$(dirname "$image_path")"

    if [[ "$dry_run" == "true" ]]; then
        return 0
    fi

    if [[ ! -d "$output_dir" ]]; then
        if ! mkdir -p "$output_dir" 2>/dev/null; then
            log_error "Output directory does not exist and cannot be created: $output_dir"
            echo "Set BASE_DIR to a writable directory, or create/chown/ACL the directory for user $(id -un)." >&2
            return 1
        fi
    fi

    if [[ ! -w "$output_dir" ]]; then
        log_error "Output directory is not writable by user $(id -un): $output_dir"
        echo "Set BASE_DIR to a writable directory, or chown/ACL $output_dir for this user." >&2
        return 1
    fi
}

confirm_overwrite_existing_image() {
    local image_path="$1"
    local force="$2"

    if [[ ! -f "$image_path" ]]; then
        return 0
    fi

    log_warn "Image already exists: $image_path"
    if [[ "$force" == "true" ]]; then
        log_warn "Overwriting existing image because --force was supplied"
        return 0
    fi

    if [[ "${CI:-}" == "true" || ! -t 0 ]]; then
        log_error "Refusing to overwrite existing image in a non-interactive run: $image_path"
        echo "Re-run with --force/-y to overwrite, or choose a different --output/BASE_DIR." >&2
        return 1
    fi

    local reply
    read -r -p "Overwrite? (y/N) " -n 1 reply
    echo
    if [[ ! $reply =~ ^[Yy]$ ]]; then
        log_info "Aborted"
        return 1
    fi
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
    # #312: shut down after install completes. Without this the installer
    # reboots into the installed system and sits idle at a login prompt
    # forever — virt-install --wait -1 and the subsequent wait-loop will
    # hang. This was masked when the script used --cdrom (rejected by
    # virt-install 1.x before the install could even start), but with
    # --location the install completes and exposes the latent bug.
    - shutdown -h now
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
    local force="$7"

    local image_name="ubuntu-server-${version}-agent.qcow2"
    local image_path="${output:-$BASE_DIR/$image_name}"
    local vm_name="build-agent-${version}"

    ensure_output_directory_writable "$image_path" "$dry_run" || return 1

    local iso_path
    if ! iso_path=$(resolve_iso_path "$version"); then
        log_error "ISO not found for Ubuntu $version"
        echo "Available ISOs in $ISO_DIR:"
        ls -la "$ISO_DIR"/*.iso 2>/dev/null || echo "  (none)"
        exit 1
    fi

    # #258: verify ISO sha256 against pinned value in iso-pins.json before
    # virt-install. Override with AIWG_SKIP_BASE_VERIFY=1 only when pinning a
    # brand-new release before SHA256SUMS is published.
    if ! verify_iso "$version" "$iso_path"; then
        log_error "ISO integrity check failed — refusing to build"
        exit 1
    fi

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

    if confirm_overwrite_existing_image "$image_path" "$force"; then
        :
    else
        return 1
    fi

    if [[ -f "$image_path" ]]; then
        sudo chattr -i "$image_path" 2>/dev/null || true
        rm -f "$image_path"
    fi

    log_info "Creating disk image..."
    qemu-img create -f qcow2 "$image_path" "$disk_size"

    local autoinstall_iso="/tmp/autoinstall-${version}-$$.iso"
    generate_autoinstall_iso "$version" "$autoinstall_iso"

    virsh destroy "$vm_name" 2>/dev/null || true
    virsh undefine "$vm_name" --nvram 2>/dev/null || true

    log_info "Starting unattended installation (this takes 10-20 minutes)..."
    echo "    You can monitor progress with: virsh console $vm_name"
    echo ""

    # #312: virt-install 1.x rejects --cdrom + --extra-args (kernel args require
    # --location or kernel install). Use --location with explicit kernel/initrd
    # paths from the Ubuntu live ISO (casper/) so autoinstall + serial console
    # kernel args are accepted. The cidata autoinstall ISO stays attached as a
    # second cdrom; cloud-init's NoCloud datasource finds it by `cidata` volume
    # label (set in generate_autoinstall_iso via genisoimage -volid cidata).
    virt-install \
        --name "$vm_name" \
        --ram "$ram" \
        --vcpus "$cpus" \
        --disk "path=$image_path,format=qcow2" \
        --location "$iso_path,kernel=casper/vmlinuz,initrd=casper/initrd" \
        --disk "path=$autoinstall_iso,device=cdrom" \
        --os-variant "ubuntu${version}" \
        --network network=default \
        --graphics none \
        --console pty,target_type=serial \
        --boot uefi \
        --extra-args "autoinstall console=ttyS0,115200n8" \
        --wait -1 \
        --noautoconsole

    log_info "Stopping post-install guest..."
    if virsh domstate "$vm_name" 2>/dev/null | grep -q running; then
        virsh shutdown "$vm_name" 2>/dev/null || true
        for _ in {1..36}; do
            if ! virsh domstate "$vm_name" 2>/dev/null | grep -q running; then
                break
            fi
            sleep 5
        done
        if virsh domstate "$vm_name" 2>/dev/null | grep -q running; then
            log_warn "Guest did not stop after ACPI shutdown; forcing power off"
            virsh destroy "$vm_name" 2>/dev/null || true
        fi
    fi

    log_info "Running post-install configuration..."

    # #561: bake the current agent-client + self-enrolling service + the
    # virtio-vsock guest transport into the base image so VMs enroll over vsock
    # host-mediated identity (ADR-023 / ADR-026) without a per-provision agent
    # deploy, and refresh the kernel/libraries/tools. Degrades gracefully when
    # the agent release binary hasn't been built yet (warn + skip the bake).
    local repo_root agent_bin agent_unit
    repo_root="$(cd "$SCRIPT_DIR/../.." && pwd)"
    agent_bin="$repo_root/agent-rs/target/release/agent-client"
    agent_unit="$repo_root/agent-rs/systemd/agent-client.service"

    local -a vc_args=(-a "$image_path")
    # Refresh kernel + libraries; add vsock/transport diagnostics.
    vc_args+=(--run-command 'apt-get update')
    vc_args+=(--run-command 'DEBIAN_FRONTEND=noninteractive apt-get -y full-upgrade')
    vc_args+=(--run-command 'DEBIAN_FRONTEND=noninteractive apt-get -y install socat iproute2')
    # Load the virtio-vsock guest transport on boot. The host supplies the
    # <vsock> device (provision-vm.sh); the guest binds this module.
    vc_args+=(--write '/etc/modules-load.d/agentic-vsock.conf:vmw_vsock_virtio_transport')
    # Assert the boot-time vsock module configuration exists. virt-customize
    # runs offline in libguestfs, so loading kernel modules here is invalid.
    vc_args+=(--run-command "test -f /etc/modules-load.d/agentic-vsock.conf && \
      grep -q '${VSOCK_GUEST_MODULE}' /etc/modules-load.d/agentic-vsock.conf")
    # Bake the agent binary + self-enrolling unit when the release binary exists.
    local agent_baked="false"
    if [[ -f "$agent_bin" && -f "$agent_unit" ]]; then
        vc_args+=(--mkdir /opt/agentic-sandbox/bin)
        vc_args+=(--copy-in "$agent_bin:/opt/agentic-sandbox/bin")
        vc_args+=(--run-command 'chmod 0755 /opt/agentic-sandbox/bin/agent-client')
        vc_args+=(--copy-in "$agent_unit:/etc/systemd/system")
        vc_args+=(--run-command 'systemctl enable agent-client.service')
        agent_baked="true"
        log_success "Baking agent-client + agent-client.service into image"
    else
        log_warn "agent-client release binary/unit not found ($agent_bin)"
        log_warn "  building image WITHOUT the agent baked in — run (cd agent-rs && cargo build --release) first"
    fi
    vc_args+=(--run-command 'systemctl enable qemu-guest-agent')
    vc_args+=(--run-command 'apt-get clean')
    vc_args+=(--run-command 'cloud-init clean --logs')
    vc_args+=(--run-command 'rm -rf /var/lib/apt/lists/*')

    virt-customize "${vc_args[@]}" || log_warn "virt-customize reported errors — verify the image before use"

    # Verify the agent actually baked in (don't ship a silently-broken image —
    # this is the #561 failure mode: an image that looks built but has no agent).
    if [[ "$agent_baked" == "true" ]]; then
        if virt-customize -a "$image_path" \
            --run-command "test -x /opt/agentic-sandbox/bin/agent-client && \
              systemctl is-enabled agent-client.service && \
              test -f /etc/modules-load.d/agentic-vsock.conf && \
              grep -q '${VSOCK_GUEST_MODULE}' /etc/modules-load.d/agentic-vsock.conf" \
            >/dev/null 2>&1; then
            log_success "Verified: agent-client present and agent-client.service enabled"
        else
            log_error "agent-client did not bake correctly — image is incomplete"
            exit 1
        fi
    fi

    virsh undefine "$vm_name" --nvram 2>/dev/null || true
    rm -f "$autoinstall_iso"

    log_info "Optimizing image size..."
    virt-sparsify --in-place "$image_path" 2>/dev/null || true

    log_info "Freezing image (read-only)..."
    chmod 444 "$image_path"
    sudo chattr +i "$image_path" 2>/dev/null || true

    # #258: record sha256 to manifest.json so provision-vm.sh can verify
    # backing-file integrity on every overlay creation.
    record_qcow2_manifest "$image_path" "$(dirname "$image_path")" || \
        log_warn "Failed to record manifest — operator must bootstrap manually"

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
    local force="false"

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -d|--disk-size) disk_size="$2"; shift 2 ;;
            -r|--ram) ram="$2"; shift 2 ;;
            -c|--cpus) cpus="$2"; shift 2 ;;
            -o|--output) output="$2"; shift 2 ;;
            -y|--yes|--force) force="true"; shift ;;
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
    build_image "$version" "$disk_size" "$ram" "$cpus" "$output" "$dry_run" "$force"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    main "$@"
fi
