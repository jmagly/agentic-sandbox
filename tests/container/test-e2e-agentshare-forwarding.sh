#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
run_e2e="$repo_root/scripts/run-e2e-tests.sh"
reprovision="$repo_root/scripts/reprovision-vm.sh"

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
