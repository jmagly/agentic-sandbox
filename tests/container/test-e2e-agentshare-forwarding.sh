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
# an XFS project quota. Both must retain the dedicated agentshare paths or the
# provisioner silently falls back to /srv/agentshare on the host root device.
for variable in AGENTSHARE_ROOT TASKS_ROOT; do
    assert_forwarded "$run_e2e" "$variable"
    assert_forwarded "$reprovision" "$variable"
done

echo "PASS: dedicated E2E agentshare paths survive both sudo env boundaries"
