#!/bin/bash
# cloud-init/common.sh - Cloud-init ISO creation and overlay disk utilities
#
# Provides functions shared across all profiles:
#   - create_cloud_init_iso  - Pack user-data/meta-data/network-config into ISO
#   - create_overlay_disk    - Create qcow2 overlay backed by a base image
#
# No required globals — all inputs are explicit parameters.

# Create cloud-init ISO
create_cloud_init_iso() {
    local cloud_init_dir="$1"
    local output_iso="$2"

    local iso_files=("$cloud_init_dir/user-data" "$cloud_init_dir/meta-data")
    if [[ -f "$cloud_init_dir/network-config" ]]; then
        iso_files+=("$cloud_init_dir/network-config")
    fi

    genisoimage -output "$output_iso" \
        -volid cidata \
        -joliet -rock \
        "${iso_files[@]}" 2>/dev/null
}

# Create overlay disk from base
create_overlay_disk() {
    local base_image="$1"
    local overlay_path="$2"
    local disk_size="$3"

    qemu-img create -f qcow2 \
        -b "$base_image" \
        -F qcow2 \
        "$overlay_path" "$disk_size"
}
