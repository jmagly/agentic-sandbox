#!/usr/bin/env bash
# Install pinned Cloud Hypervisor + Cloud Hypervisor firmware assets.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PINS_FILE="${AGENTIC_CH_PINS_FILE:-$SCRIPT_DIR/cloud-hypervisor-pins.json}"
INSTALL_ROOT="${AGENTIC_CH_INSTALL_ROOT:-/opt/agentic-sandbox/cloud-hypervisor}"
ARCH="${AGENTIC_CH_ARCH:-$(uname -m)}"
TMP_DIR=""

case "$ARCH" in
    x86_64|amd64) ARCH_KEY="x86_64" ;;
    *)
        echo "Unsupported Cloud Hypervisor install arch: $ARCH" >&2
        exit 1
        ;;
esac

read_pin() {
    local expr="$1"
    python3 - "$PINS_FILE" "$expr" <<'PY'
import json
import sys

path, expr = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
cur = data
for part in expr.split("."):
    cur = cur[part]
print(cur)
PY
}

download_and_verify() {
    local url="$1"
    local sha="$2"
    local out="$3"

    curl -fL -o "$out" "$url"
    local actual
    actual="$(sha256sum "$out" | awk '{print $1}')"
    if [[ "${actual,,}" != "${sha,,}" ]]; then
        echo "SHA256 mismatch for $url" >&2
        echo "  expected: $sha" >&2
        echo "  actual:   $actual" >&2
        return 1
    fi
}

install_file() {
    local src="$1"
    local dst="$2"
    local mode="$3"

    if install -D -m "$mode" "$src" "$dst" 2>/dev/null; then
        return 0
    fi
    sudo install -D -m "$mode" "$src" "$dst"
}

update_current_link() {
    local target="$1"
    local link="$2"

    if ln -sfnT "$target" "$link" 2>/dev/null; then
        return 0
    fi
    sudo ln -sfnT "$target" "$link"
}

main() {
    if [[ ! -f "$PINS_FILE" ]]; then
        echo "Pins file not found: $PINS_FILE" >&2
        exit 1
    fi

    local ch_version fw_version ch_base fw_base
    ch_version="$(read_pin cloud_hypervisor.version)"
    fw_version="$(read_pin edk2_firmware.version)"
    ch_base="$(read_pin cloud_hypervisor.base_url)"
    fw_base="$(read_pin edk2_firmware.base_url)"

    local ch_asset ch_sha remote_asset remote_sha fw_asset fw_sha
    ch_asset="$(read_pin cloud_hypervisor.assets.$ARCH_KEY.cloud_hypervisor.name)"
    ch_sha="$(read_pin cloud_hypervisor.assets.$ARCH_KEY.cloud_hypervisor.sha256)"
    remote_asset="$(read_pin cloud_hypervisor.assets.$ARCH_KEY.ch_remote.name)"
    remote_sha="$(read_pin cloud_hypervisor.assets.$ARCH_KEY.ch_remote.sha256)"
    fw_asset="$(read_pin edk2_firmware.assets.$ARCH_KEY.cloudhv.name)"
    fw_sha="$(read_pin edk2_firmware.assets.$ARCH_KEY.cloudhv.sha256)"

    local install_dir="$INSTALL_ROOT/$ch_version"
    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "${TMP_DIR:-}"' EXIT

    echo "Installing Cloud Hypervisor $ch_version + edk2 firmware $fw_version"
    echo "Install root: $INSTALL_ROOT"

    download_and_verify "$ch_base/$ch_asset" "$ch_sha" "$TMP_DIR/cloud-hypervisor"
    download_and_verify "$ch_base/$remote_asset" "$remote_sha" "$TMP_DIR/ch-remote"
    download_and_verify "$fw_base/$fw_asset" "$fw_sha" "$TMP_DIR/CLOUDHV.fd"

    install_file "$TMP_DIR/cloud-hypervisor" "$install_dir/bin/cloud-hypervisor" 0755
    install_file "$TMP_DIR/ch-remote" "$install_dir/bin/ch-remote" 0755
    install_file "$TMP_DIR/CLOUDHV.fd" "$install_dir/firmware/CLOUDHV.fd" 0644

    update_current_link "$install_dir" "$INSTALL_ROOT/current"

    echo "Installed:"
    echo "  $INSTALL_ROOT/current/bin/cloud-hypervisor"
    echo "  $INSTALL_ROOT/current/bin/ch-remote"
    echo "  $INSTALL_ROOT/current/firmware/CLOUDHV.fd"
    echo ""
    echo "Optional explicit environment:"
    echo "  export AGENTIC_CH_BIN=$INSTALL_ROOT/current/bin/cloud-hypervisor"
    echo "  export AGENTIC_CH_REMOTE_BIN=$INSTALL_ROOT/current/bin/ch-remote"
    echo "  export AGENTIC_CH_FIRMWARE=$INSTALL_ROOT/current/firmware/CLOUDHV.fd"
    echo "  export AGENTIC_CH_BIN_SHA256=$ch_sha"
    echo "  export AGENTIC_CH_FIRMWARE_SHA256=$fw_sha"
}

main "$@"
