#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
run_e2e="$repo_root/scripts/run-e2e-tests.sh"
reprovision="$repo_root/scripts/reprovision-vm.sh"
checkpoint="$repo_root/images/qemu/checkpoint-vm.sh"
workflow="$repo_root/.gitea/workflows/ci.yaml"

assert_forwarded() {
    local script="$1"
    local variable="$2"

    if ! grep -Fq "\"${variable}=\${${variable}:-" "$script"; then
        echo "ERROR: $(basename "$script") does not forward $variable across sudo env" >&2
        exit 1
    fi
}

# The E2E runner crosses two sudo env boundaries before provision-vm.sh assigns
# storage and an XFS project quota. Both must retain runner-specific paths or
# the provisioner silently falls back to host-root defaults. build01 keeps VM
# overlays and base images on its large /build device (#627).
for variable in AGENTSHARE_ROOT TASKS_ROOT; do
    assert_forwarded "$run_e2e" "$variable"
    assert_forwarded "$reprovision" "$variable"
done

for variable in VM_STORAGE_DIR BASE_IMAGES_DIR; do
    assert_forwarded "$run_e2e" "$variable"
    assert_forwarded "$reprovision" "$variable"
done

echo "PASS: dedicated E2E storage paths survive both sudo env boundaries"

for command in genisoimage qemu-img; do
    if ! grep -Fq "require_command $command" "$run_e2e"; then
        echo "ERROR: run-e2e-tests.sh does not fail fast when $command is missing" >&2
        exit 1
    fi
done

echo "PASS: E2E VM provisioning fails fast on missing image tools"

if ! grep -Fq 'BASE_IMAGES_DIR="${BASE_IMAGES_DIR:-${AIWG_BASE_IMAGE_DIR:-/mnt/ops/base-images}}"' "$checkpoint"; then
    echo "ERROR: checkpoint-vm.sh does not derive its selftest image from BASE_IMAGES_DIR" >&2
    exit 1
fi

if ! grep -Fq 'local BASE="${AIWG_BASE_IMAGE:-${BASE_IMAGES_DIR}/ubuntu-server-24.04-agent.qcow2}"' "$checkpoint"; then
    echo "ERROR: checkpoint-vm.sh selftest still bypasses the runner base-image contract" >&2
    exit 1
fi

if ! grep -Fq 'BASE_IMAGES_DIR="${BASE_IMAGES_DIR}"' "$workflow"; then
    echo "ERROR: CI does not forward build01's base-image directory to the privileged checkpoint selftest" >&2
    exit 1
fi

echo "PASS: libvirt checkpoint selftest honors the build01 base-image contract"
