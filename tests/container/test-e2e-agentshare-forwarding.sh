#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
run_e2e="$repo_root/scripts/run-e2e-tests.sh"
reprovision="$repo_root/scripts/reprovision-vm.sh"
checkpoint="$repo_root/images/qemu/checkpoint-vm.sh"
recovery="$repo_root/scripts/recover-libvirt-selftest.sh"
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

assert_forwarded "$reprovision" AGENT_CLIENT_SOURCE_BIN
if ! grep -Fq '"AGENT_CLIENT_SOURCE_BIN=$AGENT_BIN"' "$run_e2e"; then
    echo "ERROR: run-e2e-tests.sh does not forward the isolated agent binary across sudo env" >&2
    exit 1
fi

echo "PASS: dedicated E2E storage paths survive both sudo env boundaries"

cleanup_block="$(sed -n '/^cleanup() {/,/^}/p' "$run_e2e")"
for variable in AGENTIC_BACKEND VM_STORAGE_DIR AGENTSHARE_ROOT LIBVIRT_DEFAULT_URI; do
    if ! grep -Fq "\"${variable}=" <<<"$cleanup_block"; then
        echo "ERROR: E2E cleanup does not forward $variable to destroy-vm.sh" >&2
        exit 1
    fi
done

echo "PASS: E2E cleanup targets the same disposable substrate it provisioned"

for target_variable in MANAGEMENT_TARGET_DIR AGENT_TARGET_DIR MANAGEMENT_BIN GRPC_LOCAL_CA_BIN AGENT_BIN; do
    if ! grep -Fq "$target_variable" "$run_e2e"; then
        echo "ERROR: run-e2e-tests.sh does not derive $target_variable from the Cargo target contract" >&2
        exit 1
    fi
done

if grep -Eq 'AGENTIC_(MGMT|AGENT)_BIN="\$REPO_ROOT/(management|agent-rs)/target/release/' "$run_e2e"; then
    echo "ERROR: E2E binary overrides bypass CARGO_TARGET_DIR" >&2
    exit 1
fi

if grep -Fq 'AGENTIC_GRPC_LOCAL_CA_HELPER=$REPO_ROOT/management/target/release/' "$run_e2e"; then
    echo "ERROR: E2E CA helper bypasses CARGO_TARGET_DIR" >&2
    exit 1
fi

echo "PASS: E2E release binaries honor isolated Cargo target directories"

for script in "$run_e2e" "$reprovision"; do
    assert_forwarded "$script" AGENTIC_AGENTSHARE_READY_TIMEOUT_SECONDS
done

if ! grep -Fq 'AGENTIC_AGENTSHARE_READY_TIMEOUT_SECONDS: "600"' "$workflow"; then
    echo "ERROR: CI does not allow enough time for agentshare readiness after slow cloud-init package downloads" >&2
    exit 1
fi

if grep -Fq 'wait_for_agentshare_ready "$allocated_ip" "$SERVICE_USER" "$ephemeral_ssh_key_path" 180' \
    "$repo_root/images/qemu/provision-vm.sh"; then
    echo "ERROR: provision-vm.sh still hard-codes the agentshare readiness timeout" >&2
    exit 1
fi

echo "PASS: agentshare readiness timeout is configurable across E2E sudo boundaries"

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

if ! grep -Fq 'VM_STORAGE_DIR="${VM_STORAGE_DIR}"' "$workflow"; then
    echo "ERROR: CI does not forward build01's live VM registry to the privileged checkpoint selftest" >&2
    exit 1
fi

echo "PASS: libvirt checkpoint selftest honors the build01 base-image contract"

for host_registry in HOST_CID_REGISTRY HOST_IP_REGISTRY; do
    if ! grep -Fq "$host_registry" "$checkpoint"; then
        echo "ERROR: checkpoint selftest does not snapshot $host_registry before isolated allocation" >&2
        exit 1
    fi
done

if ! grep -Fq 'CID_START=6390' "$checkpoint" || ! grep -Fq 'CID_START=6400' "$checkpoint"; then
    echo "ERROR: checkpoint selftest source and fresh-restore CID ranges are not disjoint" >&2
    exit 1
fi

echo "PASS: libvirt checkpoint selftest avoids live E2E IP and vsock allocations"

bound_save_block="$(sed -n '/^_bound_virsh_save() {/,/^}/p' "$checkpoint")"
foreground_virsh_timeouts="$(grep -F -c 'timeout --foreground --signal=TERM' <<<"$bound_save_block" || true)"
if [[ "$foreground_virsh_timeouts" -ne 4 ]]; then
    echo "ERROR: checkpoint virsh bounds must retain PTY foreground access" >&2
    exit 1
fi
if grep -Fq 'timeout --signal=TERM' <<<"$bound_save_block"; then
    echo "ERROR: checkpoint save block contains a background-process-group timeout" >&2
    exit 1
fi

save_path="/var/tmp/chkpt-selftest/checkpoints/selftest/checkpoint.save"
"$recovery" --classify-save virsh save chkpt-selftest-base "$save_path"
"$recovery" --classify-save /usr/bin/virsh save chkpt-selftest-base "$save_path"
"$recovery" --classify-wrapper timeout --signal=TERM --kill-after=10s 120s virsh save chkpt-selftest-base "$save_path"
"$recovery" --classify-wrapper /usr/bin/timeout --foreground --signal=TERM --kill-after=10s 120s /usr/bin/virsh save chkpt-selftest-base "$save_path"

for rejected in \
    "evil save chkpt-selftest-base /var/tmp/chkpt-selftest/checkpoints/selftest/checkpoint.save" \
    "virsh save chkpt-selftest-base /var/tmp/chkpt-selftest/checkpoints/selftest/checkpointXsave" \
    "virsh save chkpt-selftest /var/tmp/chkpt-selftest/checkpoints/selftest/checkpoint.save" \
    "virsh restore chkpt-selftest-base /var/tmp/chkpt-selftest/checkpoints/selftest/checkpoint.save" \
    "virsh save unrelated /tmp/unrelated.save"; do
    # shellcheck disable=SC2086 # intentional argv splitting for classifier probes
    if "$recovery" --classify-save $rejected; then
        echo "ERROR: recovery save classifier accepted a near-match: $rejected" >&2
        exit 1
    fi
done

for rejected in \
    "timeout --signal=TERM --kill-after=10s 120s evil save chkpt-selftest-base $save_path" \
    "timeout --signal=KILL --kill-after=10s 120s virsh save chkpt-selftest-base $save_path" \
    "timeout --signal=TERM --kill-after=0s 120s virsh save chkpt-selftest-base $save_path" \
    "timeout --foreground --signal=TERM --kill-after=10s 120s virsh save chkpt-selftest-base /var/tmp/chkpt-selftest/checkpoints/selftest/checkpointXsave"; do
    # shellcheck disable=SC2086 # intentional argv splitting for classifier probes
    if "$recovery" --classify-wrapper $rejected; then
        echo "ERROR: recovery wrapper classifier accepted a near-match: $rejected" >&2
        exit 1
    fi
done

for exact_targeting_contract in \
    'pgrep -x virsh' \
    'sed -n '\''s/^PPid:[[:space:]]*//p'\'' "/proc/$pid/status"' \
    'process_matches_wrapper "$parent"' \
    'targets=("${wrappers[@]}" "${validated[@]}")' \
    'process_matches_target "$pid"' \
    'reason=target-process-remains' \
    'reason=target-pid-argv-changed'; do
    if ! grep -Fq "$exact_targeting_contract" "$recovery"; then
        echo "ERROR: recovery lost exact target contract: $exact_targeting_contract" >&2
        exit 1
    fi
done
if grep -Fq 'pgrep -f "[v]irsh save' "$recovery"; then
    echo "ERROR: checkpoint recovery discovery can confuse timeout wrappers with exact virsh candidates" >&2
    exit 1
fi

echo "PASS: libvirt checkpoint bounds and recovery remain PTY-safe and exact-process scoped"
