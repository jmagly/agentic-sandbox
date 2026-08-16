#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
BACKEND="$ROOT_DIR/images/qemu/backends/libvirt.sh"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

run_case() {
    local mode="$1"
    local calls="$WORK_DIR/$mode.calls"
    : > "$calls"

    virsh_cmd() {
        printf '%s\n' "$*" >> "$calls"
        case "$mode:$*" in
            storage-fallback:"undefine cleanup-test --nvram --remove-all-storage") return 1 ;;
            legacy-fallback:"undefine cleanup-test --nvram --remove-all-storage") return 1 ;;
            legacy-fallback:"undefine cleanup-test --nvram") return 1 ;;
            *) return 0 ;;
        esac
    }

    # shellcheck disable=SC1090
    source "$BACKEND"
    _backend_libvirt_destroy_vm cleanup-test

    grep -Fxq 'destroy cleanup-test' "$calls"
    case "$mode" in
        complete)
            grep -Fxq 'undefine cleanup-test --nvram --remove-all-storage' "$calls"
            ! grep -Fxq 'undefine cleanup-test --nvram' "$calls"
            ;;
        storage-fallback)
            grep -Fxq 'undefine cleanup-test --nvram --remove-all-storage' "$calls"
            grep -Fxq 'undefine cleanup-test --nvram' "$calls"
            ! grep -Fxq 'undefine cleanup-test' "$calls"
            ;;
        legacy-fallback)
            grep -Fxq 'undefine cleanup-test --nvram --remove-all-storage' "$calls"
            grep -Fxq 'undefine cleanup-test --nvram' "$calls"
            grep -Fxq 'undefine cleanup-test' "$calls"
            ;;
    esac
}

run_case complete
run_case storage-fallback
run_case legacy-fallback

echo "PASS libvirt UEFI destroy cleanup fallbacks"
