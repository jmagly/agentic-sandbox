#!/bin/bash
# lib/common.sh - Shared utilities: SSH key detection, base image resolution, logging fallback
#
# Provides functions for:
#   - SSH public key auto-detection
#   - Base image path resolution from shorthand names
#
# Required globals (validated at source time):
#   SSH_KEY_DIR       - Directory to search for SSH keys
#   BASE_IMAGES_DIR   - Directory containing base qcow2 images

: "${SSH_KEY_DIR:?lib/common.sh requires SSH_KEY_DIR}"
: "${BASE_IMAGES_DIR:?lib/common.sh requires BASE_IMAGES_DIR}"

# Detect OS type from base image shorthand name
detect_os_type() {
    local base="$1"
    case "$base" in
        alpine-*) echo "alpine" ;;
        ubuntu-*|*) echo "ubuntu" ;;
    esac
}

# Find SSH public key
find_ssh_key() {
    local key_file="$1"

    if [[ -n "$key_file" && -f "$key_file" ]]; then
        echo "$key_file"
        return 0
    fi

    # Auto-detect common key locations
    local keys=(
        "$SSH_KEY_DIR/id_ed25519.pub"
        "$SSH_KEY_DIR/id_rsa.pub"
        "$SSH_KEY_DIR/authorized_keys"
    )

    for key in "${keys[@]}"; do
        if [[ -f "$key" ]]; then
            echo "$key"
            return 0
        fi
    done

    log_error "No SSH public key found. Specify with --ssh-key"
    return 1
}

# Resolve base image path
resolve_base_image() {
    local base="$1"
    local image_path=""

    # Handle shorthand versions
    case "$base" in
        alpine-3.21|alpine-3.20)
            local version="${base#alpine-}"
            image_path="$BASE_IMAGES_DIR/alpine-${version}-agent.qcow2"
            ;;
        ubuntu-22.04|ubuntu-24.04|ubuntu-25.10)
            local version="${base#ubuntu-}"
            image_path="$BASE_IMAGES_DIR/ubuntu-server-${version}-agent.qcow2"
            ;;
        *.qcow2)
            if [[ "$base" == /* ]]; then
                image_path="$base"
            else
                image_path="$BASE_IMAGES_DIR/$base"
            fi
            ;;
        *)
            image_path="$BASE_IMAGES_DIR/${base}.qcow2"
            ;;
    esac

    if [[ ! -f "$image_path" ]]; then
        log_error "Base image not found: $image_path"
        echo ""
        echo "Available base images:"
        ls -la "$BASE_IMAGES_DIR"/*.qcow2 2>/dev/null || echo "  (none found)"
        echo ""
        echo "Build one with: ./build-base-image.sh 24.04"
        return 1
    fi

    echo "$image_path"
}
