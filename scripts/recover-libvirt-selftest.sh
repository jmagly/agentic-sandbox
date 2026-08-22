#!/bin/bash
# Recover the fixed-name disposable libvirt checkpoint self-test after a
# runner-side virsh save hang. Default mode is audit-only; --apply is required
# before any process, domain, or temporary self-test state is changed.
set -euo pipefail

selftest_vm="chkpt-selftest-base"
selftest_legacy_vm="chkpt-selftest"
selftest_root="/var/tmp/chkpt-selftest"
selftest_save="$selftest_root/checkpoints/selftest/checkpoint.save"

virsh_program() { [[ "$1" == "virsh" || "$1" == "/usr/bin/virsh" ]]; }
timeout_program() { [[ "$1" == "timeout" || "$1" == "/usr/bin/timeout" ]]; }

save_argv_matches() {
    [[ $# -eq 4 ]] || return 1
    virsh_program "$1" || return 1
    [[ "$2" == "save" && "$3" == "$selftest_vm" && "$4" == "$selftest_save" ]]
}

wrapper_argv_matches() {
    local offset
    timeout_program "${1:-}" || return 1
    if [[ "${2:-}" == "--foreground" ]]; then
        offset=3
        [[ $# -eq 9 ]] || return 1
    else
        offset=2
        [[ $# -eq 8 ]] || return 1
    fi
    [[ "${!offset}" == "--signal=TERM" ]] || return 1
    offset=$((offset + 1))
    [[ "${!offset}" =~ ^--kill-after=[1-9][0-9]*s$ ]] || return 1
    offset=$((offset + 1))
    [[ "${!offset}" =~ ^[1-9][0-9]*s$ ]] || return 1
    offset=$((offset + 1))
    save_argv_matches "${@:$offset}"
}

process_argv_matches() {
    local pid="$1" classifier="$2"
    local -a argv=()
    [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/$pid/cmdline" ]] || return 1
    mapfile -d '' -t argv < "/proc/$pid/cmdline"
    "$classifier" "${argv[@]}"
}

# Read NUL-separated argv instead of matching a flattened command line: the
# recovery path must never treat a near-match as a privileged kill target.
process_matches_save() { process_argv_matches "$1" save_argv_matches; }
process_matches_wrapper() { process_argv_matches "$1" wrapper_argv_matches; }
process_matches_target() { process_matches_save "$1" || process_matches_wrapper "$1"; }
process_is_zombie() { grep -Eq '^State:[[:space:]]+Z' "/proc/$1/status" 2>/dev/null; }

matching_save_pids() {
    local pid
    while IFS= read -r pid; do
        if process_matches_save "$pid"; then
            printf '%s\n' "$pid"
        fi
    done < <(pgrep -x virsh || true)
}

mode="audit"
case "${1:-}" in
    --apply)
        mode="apply"
        shift
        ;;
    --classify-save)
        shift
        if save_argv_matches "$@"; then exit 0; else exit 1; fi
        ;;
    --classify-wrapper)
        shift
        if wrapper_argv_matches "$@"; then exit 0; else exit 1; fi
        ;;
esac
[[ $# -eq 0 ]] || { echo "usage: $0 [--apply]" >&2; exit 2; }

mapfile -t validated < <(matching_save_pids)
wrappers=()
for pid in "${validated[@]}"; do
    parent="$(sed -n 's/^PPid:[[:space:]]*//p' "/proc/$pid/status")"
    if process_matches_wrapper "$parent"; then
        wrappers+=("$parent")
    fi
done

printf 'phase=libvirt-selftest-recovery mode=%s candidates=%s\n' "$mode" "${#validated[@]}"
for pid in "${validated[@]}"; do
    printf 'validated_pid=%s command=virsh-save-fixed-selftest\n' "$pid"
done
for pid in "${wrappers[@]}"; do
    printf 'validated_wrapper_pid=%s command=timeout-virsh-save-fixed-selftest\n' "$pid"
done

if [[ "$mode" == "audit" ]]; then
    exit 0
fi

targets=("${wrappers[@]}" "${validated[@]}")
for pid in "${targets[@]}"; do
    [[ -e "/proc/$pid" ]] || continue
    process_is_zombie "$pid" && continue
    process_matches_target "$pid" \
        || { echo "refusing process $pid because its exact argv changed" >&2; exit 1; }
    sudo -n kill -TERM "$pid" 2>/dev/null || true
done
for _ in {1..20}; do
    survivors=0
    for pid in "${targets[@]}"; do
        sudo -n kill -0 "$pid" 2>/dev/null && survivors=$((survivors + 1))
    done
    (( survivors == 0 )) && break
    sleep 0.25
done
for pid in "${targets[@]}"; do
    [[ -e "/proc/$pid" ]] || continue
    process_is_zombie "$pid" && continue
    process_matches_target "$pid" \
        || { echo "refusing process $pid because its exact argv changed" >&2; exit 1; }
    sudo -n kill -KILL "$pid" 2>/dev/null || true
done

# Give checkpoint-vm.sh's EXIT trap the first chance to remove its own state.
for _ in {1..20}; do
    [[ ! -e "$selftest_root" ]] && break
    sleep 0.5
done

for domain in "$selftest_vm" "$selftest_legacy_vm"; do
    if sudo -n timeout --foreground --signal=TERM --kill-after=2s 10s \
        virsh dominfo "$domain" >/dev/null 2>&1; then
        sudo -n timeout --foreground --signal=TERM --kill-after=2s 10s \
            virsh destroy "$domain" >/dev/null 2>&1 || true
        sudo -n timeout --foreground --signal=TERM --kill-after=2s 10s \
            virsh undefine "$domain" --nvram >/dev/null 2>&1 || true
    fi
done

if [[ -e "$selftest_root" ]]; then
    [[ "$selftest_root" == /var/tmp/chkpt-selftest ]] \
        || { echo "refusing unexpected self-test root: $selftest_root" >&2; exit 1; }
    sudo -n rm -rf -- "$selftest_root"
fi
[[ ! -e "$selftest_root" ]] \
    || { echo "phase=libvirt-selftest-recovery result=failed reason=selftest-root-remains" >&2; exit 1; }

mapfile -t remaining < <(matching_save_pids)
if (( ${#remaining[@]} > 0 )); then
    echo "phase=libvirt-selftest-recovery result=failed reason=save-process-remains" >&2
    exit 1
fi
for pid in "${targets[@]}"; do
    [[ -e "/proc/$pid" ]] || continue
    process_is_zombie "$pid" && continue
    if process_matches_target "$pid"; then
        echo "phase=libvirt-selftest-recovery result=failed reason=target-process-remains" >&2
    else
        echo "phase=libvirt-selftest-recovery result=failed reason=target-pid-argv-changed" >&2
    fi
    exit 1
done
domains="$(sudo -n timeout --foreground --signal=TERM --kill-after=2s 10s virsh list --all --name)" \
    || { echo "phase=libvirt-selftest-recovery result=failed reason=domain-inventory-unavailable" >&2; exit 1; }
for domain in "$selftest_vm" "$selftest_legacy_vm"; do
    if grep -Fxq "$domain" <<<"$domains"; then
        echo "phase=libvirt-selftest-recovery result=failed reason=selftest-domain-remains" >&2
        exit 1
    fi
done
printf 'phase=libvirt-selftest-recovery result=success terminated=%s cleanup=complete\n' \
    "${#targets[@]}"
