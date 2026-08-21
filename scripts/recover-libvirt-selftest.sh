#!/bin/bash
# Recover the fixed-name disposable libvirt checkpoint self-test after a
# runner-side virsh save hang. Default mode is audit-only; --apply is required
# before any process, domain, or temporary self-test state is changed.
set -euo pipefail

mode="audit"
if [[ "${1:-}" == "--apply" ]]; then
    mode="apply"
    shift
fi
[[ $# -eq 0 ]] || { echo "usage: $0 [--apply]" >&2; exit 2; }

selftest_vm="chkpt-selftest-base"
selftest_legacy_vm="chkpt-selftest"
selftest_root="/var/tmp/chkpt-selftest"
selftest_save="$selftest_root/checkpoints/selftest/checkpoint.save"
expected_re="^(/usr/bin/)?virsh save ${selftest_vm} ${selftest_save} $"

mapfile -t candidates < <(pgrep -f "[v]irsh save ${selftest_vm} ${selftest_save}" || true)
printf 'phase=libvirt-selftest-recovery mode=%s candidates=%s\n' "$mode" "${#candidates[@]}"

validated=()
for pid in "${candidates[@]}"; do
    [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/$pid/cmdline" ]] || continue
    command_line="$(tr '\0' ' ' < "/proc/$pid/cmdline")"
    if [[ "$command_line" =~ $expected_re ]]; then
        validated+=("$pid")
        printf 'validated_pid=%s command=virsh-save-fixed-selftest\n' "$pid"
    else
        echo "refusing unexpected process $pid" >&2
        exit 1
    fi
done

if [[ "$mode" == "audit" ]]; then
    exit 0
fi

for pid in "${validated[@]}"; do
    sudo -n kill -TERM "$pid" 2>/dev/null || true
done
for _ in {1..20}; do
    survivors=0
    for pid in "${validated[@]}"; do
        sudo -n kill -0 "$pid" 2>/dev/null && survivors=$((survivors + 1))
    done
    (( survivors == 0 )) && break
    sleep 0.25
done
for pid in "${validated[@]}"; do
    sudo -n kill -KILL "$pid" 2>/dev/null || true
done

# Give checkpoint-vm.sh's EXIT trap the first chance to remove its own state.
for _ in {1..20}; do
    [[ ! -e "$selftest_root" ]] && break
    sleep 0.5
done

for domain in "$selftest_vm" "$selftest_legacy_vm"; do
    if sudo -n timeout --signal=TERM --kill-after=2s 10s \
        virsh dominfo "$domain" >/dev/null 2>&1; then
        sudo -n timeout --signal=TERM --kill-after=2s 10s \
            virsh destroy "$domain" >/dev/null 2>&1 || true
        sudo -n timeout --signal=TERM --kill-after=2s 10s \
            virsh undefine "$domain" --nvram >/dev/null 2>&1 || true
    fi
done

if [[ -e "$selftest_root" ]]; then
    [[ "$selftest_root" == /var/tmp/chkpt-selftest ]] \
        || { echo "refusing unexpected self-test root: $selftest_root" >&2; exit 1; }
    sudo -n rm -rf -- "$selftest_root"
fi

if pgrep -f "[v]irsh save ${selftest_vm} ${selftest_save}" >/dev/null; then
    echo "phase=libvirt-selftest-recovery result=failed reason=save-process-remains" >&2
    exit 1
fi
printf 'phase=libvirt-selftest-recovery result=success terminated=%s cleanup=complete\n' \
    "${#validated[@]}"
