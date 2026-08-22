#!/bin/bash
# checkpoint-vm.sh - Checkpoint / fast-resume an agent VM via migrate-to-file (virsh save/restore).
#
# Implements the mechanism established by spike #639: on the q35 UEFI + virtiofs stack, `virsh save`
# is the only full RAM+device capture, and virtiofs (vhost-user) BLOCKS it
# ("Migration disabled: vhost-user backend lacks VHOST_USER_PROTOCOL_F_LOG_SHMFD feature").
# So we: unmount+detach virtiofs -> save (RAM+devices) -> persist UEFI NVRAM + the virtiofs device
# XML alongside; and on restore: restore state -> re-attach virtiofs -> guest re-mounts.
#
# Follow-up implementation for #643. Findings: docs/research/memory-snapshot-restore-spike.md
#
# Usage:
#   ./checkpoint-vm.sh save    <vm> <outfile> [--managed]
#   ./checkpoint-vm.sh restore <infile> [--name NAME] [--instance-id UUID]
#   ./checkpoint-vm.sh checkpoint --vm VM --id ID --pre-enrollment
#   ./checkpoint-vm.sh restore-checkpoint --checkpoint ID --name NAME --instance-id UUID
#   ./checkpoint-vm.sh warm-init --checkpoint ID [--checkpoint ID ...] --pool NAME
#   ./checkpoint-vm.sh warm-handoff --pool NAME --instance-id UUID
#   ./checkpoint-vm.sh selftest
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
BASE_IMAGES_DIR="${BASE_IMAGES_DIR:-${AIWG_BASE_IMAGE_DIR:-/mnt/ops/base-images}}"
AGENTSHARE_ROOT="${AGENTSHARE_ROOT:-/srv/agentshare}"
LIBVIRT_CHECKPOINT_ROOT="${LIBVIRT_CHECKPOINT_ROOT:-$VM_STORAGE_DIR/checkpoints}"
LIBVIRT_WARM_POOL_ROOT="${LIBVIRT_WARM_POOL_ROOT:-$VM_STORAGE_DIR/warm-pools}"
LIBVIRT_RESTORE_LATENCY_BUDGET_MS="${LIBVIRT_RESTORE_LATENCY_BUDGET_MS:-10000}"
LIBVIRT_RESTORE_READY_TIMEOUT_MS="${LIBVIRT_RESTORE_READY_TIMEOUT_MS:-60000}"
LIBVIRT_SAVE_TIMEOUT_SECONDS="${LIBVIRT_SAVE_TIMEOUT_SECONDS:-120}"
LIBVIRT_SAVE_KILL_AFTER_SECONDS="${LIBVIRT_SAVE_KILL_AFTER_SECONDS:-10}"
LIBVIRT_SAVE_CLEANUP_TIMEOUT_SECONDS="${LIBVIRT_SAVE_CLEANUP_TIMEOUT_SECONDS:-10}"
IP_REGISTRY="${IP_REGISTRY:-$VM_STORAGE_DIR/.ip-registry}"
IP_BASE="${IP_BASE:-192.168.122}"
IP_START="${IP_START:-201}"
IP_END="${IP_END:-254}"
CID_REGISTRY="${CID_REGISTRY:-$VM_STORAGE_DIR/.vsock-cid-registry}"
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
log()  { echo -e "${BLUE}[checkpoint]${NC} $*"; }
ok()   { echo -e "${GREEN}[ ok ]${NC} $*"; }
warn() { echo -e "${YELLOW}[warn]${NC} $*" >&2; }
die()  { echo -e "${RED}[fail]${NC} $*" >&2; exit 1; }
log_error() { warn "$*"; }
log_warn() { warn "$*"; }
log_info() { log "$*"; }
log_success() { ok "$*"; }
virsh_cmd() { virsh "$@"; }

# shellcheck source=images/qemu/lib/network.sh
source "$SCRIPT_DIR/lib/network.sh"

usage() {
    cat <<EOF
Usage:
  $0 save    <vm> <outfile> [--managed]   Checkpoint a running VM to <outfile>
  $0 restore <infile> [--name NAME]        Restore a VM from a checkpoint
  $0 checkpoint --vm VM --id ID --pre-enrollment
                                                Capture a named clean checkpoint
  $0 restore-checkpoint --checkpoint ID --name NAME --instance-id UUID
                                                Restore with fresh identity/CID
  $0 warm-init --checkpoint ID [--checkpoint ID ...] --pool NAME
                                                Reserve distinct saved VMs as slots
  $0 warm-handoff --pool NAME --instance-id UUID
                                                Claim, restore, and enroll one slot
  $0 selftest                              Build a throwaway VM and round-trip it

save writes three artifacts:
  <outfile>              QEMU migrate-to-file image (RAM + device state)
  <outfile>.virtiofs.xml virtiofs <filesystem> device XML (for re-attach on restore)
  <outfile>.nvram        copy of the per-VM UEFI NVRAM (if the domain uses pflash)

Notes (from spike #639):
  - virtiofs must be detached before save; this tool unmounts (best-effort via guest agent)
    then detaches, then saves, then re-attaches + remounts on restore.
  - internal qcow2 snapshots (savevm) do NOT work on this profile (memfd + UEFI pflash).
  - restore-to-usable is seconds, not sub-second; see #644 (Cloud Hypervisor) for sub-second.
  - a checkpoint contains everything in guest RAM VERBATIM (mTLS key #617, OAuth, bootstrap
    token #619). Prefer snapshotting a pre-enrollment CLEAN base. If a secret-bearing checkpoint
    is unavoidable, seal it at rest with snapshot-seal.sh (#645):
      ./snapshot-seal.sh seal --key <keyfile> --out ckpt.gpg <outfile> <outfile>.nvram <outfile>.virtiofs.xml
EOF
}

# --- helpers ---------------------------------------------------------------
json_escape() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    value="${value//$'\n'/\\n}"
    printf '%s' "$value"
}
ms_now() {
    # Elapsed-time budgets must not depend on runner wall-clock corrections.
    # Linux exposes monotonic uptime with sub-second precision.
    awk '{printf "%.0f\n", $1 * 1000}' /proc/uptime
}
validate_identifier() {
    local label="$1" value="$2"
    [[ "$value" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]] \
        || die "$label must match [A-Za-z0-9][A-Za-z0-9._-]{0,127}"
}
checkpoint_path_for() {
    local id="$1"
    validate_identifier "checkpoint id" "$id"
    printf '%s/%s/checkpoint.save' "$LIBVIRT_CHECKPOINT_ROOT" "$id"
}
checkpoint_state_for() { printf '%s.state.json' "$1"; }
checkpoint_registry_lock() {
    mkdir -p "$LIBVIRT_CHECKPOINT_ROOT"
    printf '%s/.checkpoint-registry.lock' "$LIBVIRT_CHECKPOINT_ROOT"
}
pool_dir_for() {
    local pool="$1"
    validate_identifier "pool name" "$pool"
    printf '%s/%s' "$LIBVIRT_WARM_POOL_ROOT" "$pool"
}
_agent_ok()   { virsh qemu-agent-command "$1" '{"execute":"guest-ping"}' >/dev/null 2>&1; }
_wait_agent() { local vm=$1 lim=${2:-90} t=0; while ! _agent_ok "$vm"; do sleep 1; t=$((t+1)); [ $t -ge "$lim" ] && return 1; done; return 0; }
_running()    { [ "$(virsh domstate "$1" 2>/dev/null)" = "running" ]; }

_domain_is_persistent() {
    local info state
    info="$(virsh dominfo "$1" 2>/dev/null)" || return 1
    state="$(sed -n 's/^Persistent:[[:space:]]*//p' <<<"$info" | tr -d '\r')"
    [[ "$state" == "yes" ]]
}

_wait_domain_persistent() {
    local name="$1" attempts="${2:-20}" delay="${3:-0.1}" confirmations="${4:-3}"
    local attempt consecutive=0
    for ((attempt = 1; attempt <= attempts; attempt++)); do
        if _domain_is_persistent "$name"; then
            consecutive=$((consecutive + 1))
            (( consecutive >= confirmations )) && return 0
        else
            consecutive=0
        fi
        sleep "$delay"
    done
    return 1
}

_libvirt_qemu_group() {
    if [[ -n "${LIBVIRT_QEMU_GROUP:-}" ]] && getent group "$LIBVIRT_QEMU_GROUP" >/dev/null; then
        printf '%s\n' "$LIBVIRT_QEMU_GROUP"
    elif id -gn libvirt-qemu >/dev/null 2>&1; then
        id -gn libvirt-qemu
    elif getent group kvm >/dev/null; then
        printf '%s\n' kvm
    elif getent group qemu >/dev/null; then
        printf '%s\n' qemu
    else
        return 1
    fi
}

_grant_restore_storage_access() {
    local vm_dir="$1" inbox="$2" disk="$3" nvram="$4" qemu_group
    qemu_group="$(_libvirt_qemu_group)" \
        || die "could not determine libvirt QEMU group; set LIBVIRT_QEMU_GROUP"
    chgrp "$qemu_group" "$vm_dir" "$inbox" "$disk" "$nvram" \
        || die "could not grant libvirt group ownership to restore artifacts"
    chmod 750 "$vm_dir"
    chmod 770 "$inbox"
    chmod 640 "$disk" "$nvram"
}

_guest_exec_capture() {
    local vm="$1" script="$2" request response pid status encoded
    command -v jq >/dev/null 2>&1 || die "jq is required for clean-base verification"
    request="$(jq -nc --arg script "$script" '{execute:"guest-exec",arguments:{path:"/bin/bash",arg:["-c",$script],"capture-output":true}}')"
    response="$(virsh qemu-agent-command "$vm" "$request")" || return 1
    pid="$(printf '%s' "$response" | jq -r '.return.pid // empty')"
    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    local attempt
    for attempt in $(seq 1 300); do
        status="$(virsh qemu-agent-command "$vm" "{\"execute\":\"guest-exec-status\",\"arguments\":{\"pid\":$pid}}")" || return 1
        if [[ "$(printf '%s' "$status" | jq -r '.return.exited // false')" == "true" ]]; then
            [[ "$(printf '%s' "$status" | jq -r '.return.exitcode // 1')" == "0" ]] || return 1
            encoded="$(printf '%s' "$status" | jq -r '.return["out-data"] // empty')"
            [[ -z "$encoded" ]] || printf '%s' "$encoded" | base64 -d
            return 0
        fi
        sleep 0.1
        [[ "$attempt" -lt 300 ]] || return 1
    done
    return 1
}

_prepare_clean_base() {
    local vm="$1" evidence
    # The guest script is intentionally single-quoted so host variables and
    # command substitutions cannot expand before QEMU guest-agent delivery.
    # shellcheck disable=SC2016
    evidence="$(_guest_exec_capture "$vm" 'set -euo pipefail
systemctl stop agent-client.service 2>/dev/null || true
systemctl start agent-client-restore-bootstrap.path agent-client-restore-bootstrap.timer 2>/dev/null || true
systemctl restart agent-client-restore-bootstrap-watcher.service 2>/dev/null || true
systemctl is-active --quiet agent-client.service && exit 20
pgrep -x agent-client >/dev/null && exit 21
for path in /etc/agentic-sandbox/grpc-mtls /etc/agentic-sandbox/tls /run/agentic-sandbox/bootstrap-tls; do
  if [[ -d "$path" ]] && find "$path" -type f -print -quit | grep -q .; then exit 22; fi
done
if [[ -f /etc/agentic-sandbox/agent.env ]] && grep -Eq "^(AGENT_BOOTSTRAP_TOKEN|AGENT_SECRET|OAUTH_|.*TOKEN=|AGENT_GRPC_TLS_(CA|CERT|KEY)=)" /etc/agentic-sandbox/agent.env; then exit 23; fi
[[ ! -e /mnt/inbox/restore-bootstrap.env ]] || exit 24
for credentials in /root/.claude/.credentials.json /home/*/.claude/.credentials.json; do
  [[ ! -s "$credentials" ]] || exit 25
done
printf "{\"schema\":1,\"agent_inactive\":true,\"no_tls_identity\":true,\"no_bootstrap_or_oauth\":true}\n"')" \
        || die "guest $vm failed pre-enrollment clean-base verification"
    printf '%s' "$evidence" | jq -e '.schema == 1 and .agent_inactive and .no_tls_identity and .no_bootstrap_or_oauth' >/dev/null \
        || die "guest $vm returned invalid clean-base evidence"
}

# Extract each <filesystem>...</filesystem> block (virtiofs) from a domain's live XML.
_virtiofs_blocks() {
    virsh dumpxml "$1" 2>/dev/null | awk '
        /<filesystem/{cap=1}
        cap{buf=buf $0 ORS}
        /<\/filesystem>/{
            if(cap && buf ~ /<driver[^>]+type=[^>]*virtiofs/) printf "%s\036", buf
            buf=""; cap=0
        }'
}

_detach_virtiofs() {
    local vm="$1" blocks="$2" n=0 block="" attempt detached
    while IFS= read -r -d $'\036' block; do
        [[ -n "${block//[$'\t\r\n ']/}" ]] || continue
        detached=false
        for attempt in {1..20}; do
            if virsh detach-device "$vm" <(printf '%s' "$block") --live >/dev/null 2>&1; then
                n=$((n + 1))
                detached=true
                break
            fi
            sleep 0.5
        done
        [[ "$detached" == true ]] || die "failed to detach a virtiofs device from $vm after 10s"
    done < <(printf '%s' "$blocks")

    [[ -z "$(_virtiofs_blocks "$vm")" ]] \
        || die "virtiofs devices remain attached to $vm after detach"
    printf '%s\n' "$n"
}
# Path of the per-VM UEFI NVRAM file, if any.
_nvram_path() { virsh dumpxml "$1" 2>/dev/null | sed -n "s/.*<nvram[^>]*>\(.*\)<\/nvram>.*/\1/p" | head -1; }

# Best-effort: unmount every virtiofs mount inside the guest (so detach won't EBUSY).
_guest_umount_virtiofs() {
    local vm=$1
    _agent_ok "$vm" || { warn "no guest agent; skipping in-guest unmount for $vm"; return 0; }
    # shellcheck disable=SC2016
    virsh qemu-agent-command "$vm" \
      '{"execute":"guest-exec","arguments":{"path":"/bin/sh","arg":["-c","for m in $(awk \"/ virtiofs /{print \\$2}\" /proc/mounts); do umount -l \"$m\"; done"]}}' \
      >/dev/null 2>&1 || true
    sleep 1
}
# Best-effort: remount known agentshare tags after re-attach.
_guest_remount_virtiofs() {
    local vm=$1
    _agent_ok "$vm" || return 0
    virsh qemu-agent-command "$vm" \
      '{"execute":"guest-exec","arguments":{"path":"/bin/sh","arg":["-c","mountpoint -q /mnt/global || (mkdir -p /mnt/global && mount -t virtiofs agentglobal /mnt/global -o ro 2>/dev/null); mountpoint -q /mnt/inbox || (mkdir -p /mnt/inbox && mount -t virtiofs agentinbox /mnt/inbox 2>/dev/null); true"]}}' \
      >/dev/null 2>&1 || true
}

_remove_partial_checkpoint() {
    local out="$1"
    rm -f -- "$out" "$out.domain.xml" "$out.virtiofs.xml" "$out.nvram" \
        "$out.metadata.json" "$(checkpoint_state_for "$out")"
}

_bound_virsh_save() {
    local vm="$1" out="$2" managed="$3" phase rc=0 state cleanup="not-needed"
    [[ "$LIBVIRT_SAVE_TIMEOUT_SECONDS" =~ ^[1-9][0-9]*$ ]] \
        || die "LIBVIRT_SAVE_TIMEOUT_SECONDS must be a positive integer"
    [[ "$LIBVIRT_SAVE_KILL_AFTER_SECONDS" =~ ^[1-9][0-9]*$ ]] \
        || die "LIBVIRT_SAVE_KILL_AFTER_SECONDS must be a positive integer"
    [[ "$LIBVIRT_SAVE_CLEANUP_TIMEOUT_SECONDS" =~ ^[1-9][0-9]*$ ]] \
        || die "LIBVIRT_SAVE_CLEANUP_TIMEOUT_SECONDS must be a positive integer"
    command -v timeout >/dev/null 2>&1 || die "timeout is required for bounded libvirt saves"

    # Gitea host-executor steps have a controlling PTY. Without --foreground,
    # timeout creates a background process group; virsh can then receive a
    # job-control stop while touching that PTY, which also stops the timer.
    if [[ "$managed" == true ]]; then
        phase="virsh-managedsave"
        timeout --foreground --signal=TERM --kill-after="${LIBVIRT_SAVE_KILL_AFTER_SECONDS}s" \
            "${LIBVIRT_SAVE_TIMEOUT_SECONDS}s" virsh managedsave "$vm" >/dev/null || rc=$?
    else
        phase="virsh-save"
        timeout --foreground --signal=TERM --kill-after="${LIBVIRT_SAVE_KILL_AFTER_SECONDS}s" \
            "${LIBVIRT_SAVE_TIMEOUT_SECONDS}s" virsh save "$vm" "$out" >/dev/null || rc=$?
    fi
    (( rc == 0 )) && return 0

    _remove_partial_checkpoint "$out"
    if (( rc == 124 || rc == 137 )); then
        state="$(timeout --foreground --signal=TERM --kill-after=2s \
            "${LIBVIRT_SAVE_CLEANUP_TIMEOUT_SECONDS}s" virsh domstate "$vm" 2>/dev/null || true)"
        case "$state" in
            running|paused|blocked|"pmsuspended")
                if timeout --foreground --signal=TERM --kill-after=2s \
                    "${LIBVIRT_SAVE_CLEANUP_TIMEOUT_SECONDS}s" virsh destroy "$vm" >/dev/null 2>&1; then
                    cleanup="forced-shutoff"
                else
                    cleanup="incomplete"
                fi
                ;;
            "shut off"|"shutoff") cleanup="already-shutoff" ;;
            "") cleanup="domain-unavailable" ;;
            *) cleanup="state-${state// /-}" ;;
        esac
        warn "phase=$phase timeout_seconds=$LIBVIRT_SAVE_TIMEOUT_SECONDS cleanup=$cleanup partial_checkpoint_removed=$out"
        return 124
    fi

    warn "phase=$phase exit_status=$rc cleanup=partial-checkpoint-removed path=$out"
    return "$rc"
}

# --- save ------------------------------------------------------------------
cmd_save() {
    local vm="${1:-}" out="${2:-}"; shift 2 || true
    local managed=false pre_enrollment=false checkpoint_id=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --managed) managed=true; shift ;;
            --pre-enrollment) pre_enrollment=true; shift ;;
            --id) checkpoint_id="$2"; shift 2 ;;
            *) die "unknown save argument: $1" ;;
        esac
    done
    if [[ -z "$vm" || -z "$out" ]]; then
        usage
        die "save needs <vm> <outfile>"
    fi
    _running "$vm" || die "domain '$vm' is not running"
    mkdir -p "$(dirname "$out")"

    if [[ "$pre_enrollment" == true ]]; then
        _prepare_clean_base "$vm"
    fi

    # 1. persist virtiofs device XML + UEFI NVRAM before we tear anything down
    virsh dumpxml "$vm" > "$out.domain.xml"
    local blocks; blocks="$(_virtiofs_blocks "$vm")"
    : > "$out.virtiofs.xml"
    if [ -n "$blocks" ]; then printf '%s' "$blocks" | tr '\036' '\n' > "$out.virtiofs.xml"; fi
    local nvram; nvram="$(_nvram_path "$vm")"
    if [ -n "$nvram" ] && [ -r "$nvram" ]; then cp -f "$nvram" "$out.nvram"; ok "saved NVRAM ($(du -h "$out.nvram"|cut -f1))"; fi

    # 2. quiesce + detach virtiofs (required: virtiofs blocks migrate-to-file)
    if [ -n "$blocks" ]; then
        _guest_umount_virtiofs "$vm"
        local n
        n="$(_detach_virtiofs "$vm" "$blocks")"
        ok "detached $n virtiofs device(s)"
    fi

    # 3. capture RAM + device state
    local t0 t1 save_rc=0; t0=$(date +%s.%N)
    if $managed; then
        _bound_virsh_save "$vm" "$out" true || save_rc=$?
        (( save_rc == 0 )) || die "managedsave failed with status $save_rc"
        # managedsave stores under libvirt's save dir; copy out for portability
        local msf="/var/lib/libvirt/qemu/save/${vm}.save"
        if [[ -r "$msf" ]]; then
            cp -f "$msf" "$out" 2>/dev/null \
                || warn "managedsave image not directly readable; state kept in libvirt"
        else
            warn "managedsave image not directly readable; state kept in libvirt"
        fi
    else
        _bound_virsh_save "$vm" "$out" false || save_rc=$?
        (( save_rc == 0 )) || die "virsh save failed with status $save_rc"
    fi
    t1=$(date +%s.%N)
    sync
    local sz; sz=$(stat -c %s "$out" 2>/dev/null || echo 0)
    ok "checkpoint written: $out ($((sz/1048576)) MiB) in $(awk "BEGIN{printf \"%.2f\",$t1-$t0}")s"
    log "sidecars: $out.virtiofs.xml  $out.nvram"
    local duration_ms source_disk metadata_tmp
    duration_ms="$(awk "BEGIN{printf \"%.0f\",($t1-$t0)*1000}")"
    source_disk="$(sed -n "/<disk .*device=['\"]disk['\"]/,/<\/disk>/ s/.*<source file=['\"]\([^'\"]*\)['\"].*/\1/p" "$out.domain.xml" | head -n1)"
    metadata_tmp="$(mktemp "$(dirname "$out")/.metadata.XXXXXX")"
    jq -n \
        --arg checkpoint_id "${checkpoint_id:-$(basename "$(dirname "$out")")}" \
        --arg source_vm "$vm" \
        --arg source_disk "$source_disk" \
        --argjson pre_enrollment "$pre_enrollment" \
        --argjson size_bytes "$sz" \
        --argjson save_duration_ms "$duration_ms" \
        '{schema:1,backend:"libvirt",checkpoint_id:$checkpoint_id,source_vm:$source_vm,source_disk:$source_disk,pre_enrollment:$pre_enrollment,size_bytes:$size_bytes,save_duration_ms:$save_duration_ms}' \
        > "$metadata_tmp"
    chmod 600 "$metadata_tmp" "$out" "$out.domain.xml" "$out.virtiofs.xml" "$out.nvram" 2>/dev/null || true
    mv -fT -- "$metadata_tmp" "$out.metadata.json"
    printf '{"checkpoint_id":"%s","path":"%s","pre_enrollment":%s,"size_bytes":%s,"duration_ms":%s}\n' \
        "$(json_escape "${checkpoint_id:-$(basename "$(dirname "$out")")}")" \
        "$(json_escape "$out")" "$pre_enrollment" "$sz" "$duration_ms"
}

# --- restore enrollment + identity -----------------------------------------
load_bootstrap_stdin() {
    LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD=""
    if [[ "${LIBVIRT_BOOTSTRAP_STDIN:-}" != "1" ]]; then
        return 0
    fi
    command -v jq >/dev/null 2>&1 || die "jq is required for restore bootstrap stdin"
    IFS= read -r LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD || die "restore bootstrap stdin was empty"
    printf '%s' "$LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD" | jq -e '.single | type == "object"' >/dev/null \
        || die "restore bootstrap stdin must contain a single object"
}

_stage_restore_bootstrap() {
    local name="$1" instance_id="$2" inbox="$3" ip="$4" mac="$5"
    RESTORE_BOOTSTRAP_ISSUED=false
    RESTORE_BOOTSTRAP_SPIFFE=""
    [[ -n "${LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD:-}" ]] || return 0
    local token spiffe expires tls_dir enrollment_url ca_pem tmp ca_path
    token="$(printf '%s' "$LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD" | jq -r '.single.token // empty')"
    spiffe="$(printf '%s' "$LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD" | jq -r '.single.spiffe_id // empty')"
    expires="$(printf '%s' "$LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD" | jq -r '.single.expires_at_unix_ms // empty')"
    tls_dir="$(printf '%s' "$LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD" | jq -r '.single.tls_dir // empty')"
    enrollment_url="$(printf '%s' "$LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD" | jq -r '.single.enrollment_url // empty')"
    ca_pem="$(printf '%s' "$LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD" | jq -r '.single.ca_pem // empty')"
    local envelope_instance
    envelope_instance="$(printf '%s' "$LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD" | jq -r '.single.instance_id // empty')"
    [[ -z "$envelope_instance" || "$envelope_instance" == "$instance_id" ]] \
        || die "bootstrap instance_id does not match restore instance_id"
    [[ -n "$token" && -n "$spiffe" && -n "$ca_pem" ]] \
        || die "restore bootstrap requires token, spiffe_id, and ca_pem"
    [[ "$enrollment_url" == https://* ]] || die "restore bootstrap enrollment URL must use HTTPS"
    mkdir -p "$inbox"
    chmod 700 "$inbox"
    ca_path="$inbox/restore-bootstrap-ca.pem"
    tmp="$(mktemp "$inbox/.restore-bootstrap.env.XXXXXX")"
    (
        umask 077
        printf '%s\n' "$ca_pem" > "$ca_path"
        cat > "$tmp" <<EOF
AGENT_TRANSPORT=auto
AGENT_ID=$instance_id
AGENT_INSTANCE_ID=$instance_id
AGENT_RESTORE_GRPC_SERVER=${AGENTIC_VM_RESTORE_GRPC_SERVER:-host.internal:8123}
AGENT_RESTORE_HOST_ADDRESS=${AGENTIC_VM_HOST_ADDRESS:-192.168.122.1}
AGENT_RESTORE_MAC_ADDRESS=$mac
AGENT_RESTORE_IP_ADDRESS=$ip
AGENT_RESTORE_IP_PREFIX=${AGENTIC_VM_RESTORE_IP_PREFIX:-24}
AGENT_BOOTSTRAP_TOKEN=$token
AGENT_BOOTSTRAP_SPIFFE_ID=$spiffe
AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=$expires
AGENT_BOOTSTRAP_TLS_DIR=${tls_dir:-/etc/agentic-sandbox/grpc-mtls}
AGENT_BOOTSTRAP_ENROLLMENT_URL=$enrollment_url
AGENT_BOOTSTRAP_CA=/mnt/inbox/restore-bootstrap-ca.pem
EOF
        chmod 600 "$ca_path" "$tmp"
    )
    mv -fT -- "$tmp" "$inbox/restore-bootstrap.env"
    rm -f -- "$inbox/restore-enrollment-ready.json"
    RESTORE_BOOTSTRAP_ISSUED=true
    RESTORE_BOOTSTRAP_SPIFFE="$spiffe"
}

_rewrite_domain_xml() {
    local input="$1" output="$2" name="$3" cid="$4" disk="$5" nvram="$6"
    awk -v name="$name" -v cid="$cid" -v disk="$disk" -v nvram="$nvram" '
        {
            gsub(/<name>[^<]*<\/name>/, "<name>" name "</name>")
            if ($0 ~ /<disk / && $0 ~ /device=.disk./) in_disk=1
            if (in_disk && $0 ~ /<source file=/) {
                sub(/file=\047[^\047]*\047/, "file=\047" disk "\047")
                sub(/file=\042[^\042]*\042/, "file=\042" disk "\042")
            }
            if ($0 ~ /<\/disk>/) in_disk=0
            if ($0 ~ /<nvram/) sub(/>[^<]*<\/nvram>/, ">" nvram "</nvram>")
            if ($0 ~ /<cid /) {
                sub(/address=\047[^\047]*\047/, "address=\047" cid "\047")
                sub(/address=\042[^\042]*\042/, "address=\042" cid "\042")
                sub(/auto=\047yes\047/, "auto=\047no\047")
                sub(/auto=\042yes\042/, "auto=\042no\042")
            }
            print
        }
    ' "$input" > "$output"
}

_rewrite_virtiofs_sidecar() {
    local input="$1" output="$2" inbox="$3"
    : > "$output"
    local block="" line
    while IFS= read -r line || [[ -n "$line" ]]; do
        block+="$line"$'\n'
        if [[ "$line" == *"</filesystem>"* ]]; then
            if [[ "$block" == *"dir='agentinbox'"* || "$block" == *'dir="agentinbox"'* ]]; then
                block="$(printf '%s' "$block" | sed -E \
                    -e "s#<source dir='[^']*'#<source dir='$inbox'#" \
                    -e "s#<source dir=\"[^\"]*\"#<source dir=\"$inbox\"#")"
                block+=$'\n'
            fi
            printf '%s' "$block" >> "$output"
            block=""
        fi
    done < "$input"
}

_trigger_restore_bootstrap() {
    local name="$1"
    [[ "$RESTORE_BOOTSTRAP_ISSUED" == true ]] || return 0
    local request
    request='{"execute":"guest-exec","arguments":{"path":"/bin/systemctl","arg":["start","agent-client-restore-bootstrap.service"]}}'
    virsh qemu-agent-command "$name" "$request" >/dev/null \
        || die "could not activate restore bootstrap through the guest agent"
}

_wait_for_enrollment_ready() {
    local inbox="$1" expected_spiffe="$2" start_ms="$3"
    local ack="$inbox/restore-enrollment-ready.json"
    local deadline=$(( $(ms_now) + LIBVIRT_RESTORE_READY_TIMEOUT_MS ))
    while (( $(ms_now) <= deadline )); do
        if [[ ! -e "$inbox/restore-bootstrap.env" && -f "$ack" \
            && "$(stat -c '%a' "$ack" 2>/dev/null)" == "600" ]] \
            && jq -e --arg spiffe "$expected_spiffe" \
                '.schema == 1 and .spiffe_id == $spiffe and .tls_materialized == true and (.certificate_sha256 | test("^[0-9a-f]{64}$"))' \
                "$ack" >/dev/null 2>&1; then
            RESTORE_READY_DURATION_MS=$(( $(ms_now) - start_ms ))
            return 0
        fi
        sleep 0.05
    done
    die "restored guest did not provide a validated enrollment acknowledgement within ${LIBVIRT_RESTORE_READY_TIMEOUT_MS}ms"
}

# --- restore ---------------------------------------------------------------
cmd_restore() {
    local in="${1:-}"; shift || true
    local name="" instance_id="" checkpoint_id="" wait_enrollment_ready=false
    while [ $# -gt 0 ]; do
        case "$1" in
            --name) name="$2"; shift 2 ;;
            --instance-id) instance_id="$2"; shift 2 ;;
            --checkpoint-id) checkpoint_id="$2"; shift 2 ;;
            --wait-enrollment-ready) wait_enrollment_ready=true; shift ;;
            *) die "unknown restore argument: $1" ;;
        esac
    done
    # NB: virsh save writes the image root-owned 0600; libvirtd (root) reads it on restore,
    # so we test existence, not our own read access.
    if [[ -z "$in" || ! -e "$in" ]]; then
        usage
        die "restore needs an existing <infile>"
    fi

    if [[ -n "$instance_id" ]]; then
        [[ "${LIBVIRT_INTERNAL_RESTORE:-0}" == "1" ]] \
            || die "fresh restore must use restore-checkpoint or warm-handoff"
        [[ -n "$name" ]] || die "fresh restore requires --name"
        validate_identifier "VM name" "$name"
        [[ "$instance_id" =~ ^[0-9a-fA-F-]{32,36}$ ]] || die "instance-id must be a UUID"
        [[ -r "$in.metadata.json" ]] || die "fresh restore requires checkpoint metadata"
        jq -e '.schema == 1 and .backend == "libvirt" and .pre_enrollment == true' "$in.metadata.json" >/dev/null \
            || die "fresh restore requires an attested pre-enrollment checkpoint"
        local source_vm
        source_vm="$(jq -r '.source_vm // empty' "$in.metadata.json")"
        [[ "$name" == "$source_vm" ]] \
            || die "libvirt saved state must restore with its source domain name: $source_vm"
        virsh dominfo "$name" >/dev/null 2>&1 && die "domain already exists: $name"
        local vm_dir="$VM_STORAGE_DIR/$name" inbox="$AGENTSHARE_ROOT/$name-inbox"
        [[ ! -e "$vm_dir" || -d "$vm_dir" ]] || die "VM storage path is not a directory: $vm_dir"
        [[ ! -e "$inbox" || -d "$inbox" ]] || die "VM inbox path is not a directory: $inbox"
        local vm_dir_created=false inbox_created=false
        if [[ ! -d "$vm_dir" ]]; then mkdir -p "$vm_dir"; vm_dir_created=true; fi
        if [[ ! -d "$inbox" ]]; then mkdir -p "$inbox"; inbox_created=true; fi
        [[ "$vm_dir_created" != true ]] || chmod 700 "$vm_dir"
        [[ "$inbox_created" != true ]] || chmod 700 "$inbox"
        local cid="" ip="" mac network="${LIBVIRT_NETWORK:-default}" prior_ip=""
        prior_ip="$(get_vm_allocated_ip "$name")"
        local restore_tag="${instance_id//-/}"
        local child_disk="$vm_dir/${name}-restore-${restore_tag}.qcow2"
        local child_nvram="$vm_dir/${name}-restore-${restore_tag}_VARS.fd"
        local source_xml="$vm_dir/checkpoint-source-${restore_tag}.xml"
        local restore_xml="$vm_dir/restore-${restore_tag}.xml"
        local fs_xml="$vm_dir/virtiofs-${restore_tag}.xml"
        local persistent_xml="$vm_dir/domain-restored-${restore_tag}.xml"
        local vm_info="$vm_dir/vm-info.json" domain_xml="$vm_dir/domain.xml"
        [[ ! -e "$child_disk" && ! -e "$child_nvram" ]] \
            || die "restore artifacts already exist for instance $instance_id"
        local vm_info_backup="" domain_xml_backup=""
        if [[ -e "$vm_info" ]]; then
            vm_info_backup="$(mktemp "$vm_dir/.vm-info.pre-restore.XXXXXX")"
            cp -p -- "$vm_info" "$vm_info_backup"
        fi
        if [[ -e "$domain_xml" ]]; then
            domain_xml_backup="$(mktemp "$vm_dir/.domain.pre-restore.XXXXXX")"
            cp -p -- "$domain_xml" "$domain_xml_backup"
        fi
        local restore_succeeded=false
        _fresh_restore_cleanup() {
            if [[ "$restore_succeeded" != true ]]; then
                virsh destroy "$name" >/dev/null 2>&1 || true
                virsh undefine "$name" --nvram >/dev/null 2>&1 || true
                [[ -z "$cid" ]] || remove_cid_allocation "$name" || true
                [[ -z "$ip" || -n "$prior_ip" ]] \
                    || remove_dhcp_reservation "$network" "$name" "$mac" "$ip" || true
                rm -f -- "$child_disk" "$child_nvram" "$source_xml" "$restore_xml" \
                    "$fs_xml" "$persistent_xml" "$inbox/restore-bootstrap.env" \
                    "$inbox/restore-bootstrap-ca.pem" "$inbox/restore-enrollment-ready.json"
                if [[ -n "$vm_info_backup" ]]; then mv -f -- "$vm_info_backup" "$vm_info"; else rm -f -- "$vm_info"; fi
                if [[ -n "$domain_xml_backup" ]]; then mv -f -- "$domain_xml_backup" "$domain_xml"; else rm -f -- "$domain_xml"; fi
                [[ "$inbox_created" != true ]] || rmdir --ignore-fail-on-non-empty "$inbox" 2>/dev/null || true
                [[ "$vm_dir_created" != true ]] || rmdir --ignore-fail-on-non-empty "$vm_dir" 2>/dev/null || true
            fi
        }
        trap _fresh_restore_cleanup EXIT
        if ! virsh save-image-dumpxml "$in" > "$source_xml" 2>/dev/null; then
            cp "$in.domain.xml" "$source_xml"
        fi
        mac="$(sed -n "/<interface /,/<\/interface>/ s/.*<mac address=['\"]\([^'\"]*\)['\"].*/\1/p" "$source_xml" | head -n1)"
        [[ "$mac" =~ ^([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}$ ]] \
            || die "checkpoint source network interface has no valid MAC"
        cid="$(allocate_cid_for_vm "$name" "$instance_id")" || die "failed to allocate fresh vsock CID"
        ip="$(allocate_ip_for_vm "$name" "$network")" || die "failed to allocate fresh VM IP"
        add_dhcp_reservation "$network" "$name" "$mac" "$ip"
        local source_disk
        source_disk="$(jq -r '.source_disk // empty' "$in.metadata.json")"
        [[ -r "$source_disk" ]] || die "checkpoint source disk is unavailable: $source_disk"
        qemu-img create -f qcow2 -F qcow2 -b "$source_disk" "$child_disk" >/dev/null
        [[ -r "$in.nvram" ]] || die "fresh restore requires the NVRAM sidecar"
        cp --reflink=auto "$in.nvram" "$child_nvram"
        _rewrite_domain_xml "$source_xml" "$restore_xml" "$name" "$cid" "$child_disk" "$child_nvram"
        _rewrite_virtiofs_sidecar "$in.virtiofs.xml" "$fs_xml" "$inbox"
        _stage_restore_bootstrap "$name" "$instance_id" "$inbox" "$ip" "$mac"
        _grant_restore_storage_access "$vm_dir" "$inbox" "$child_disk" "$child_nvram"
        jq -n --arg instance_id "$instance_id" --arg name "$name" --arg ip "$ip" --arg mac "$mac" \
            --arg checkpoint_id "$checkpoint_id" --argjson cid "$cid" \
            '{instance_id:$instance_id,name:$name,backend:"libvirt",ip:$ip,mac:$mac,vsock_cid:$cid,source_checkpoint:$checkpoint_id,enroll_on_restore:true}' \
            > "$vm_info"
        chmod 600 "$vm_info" "$restore_xml" "$fs_xml"
        local start_ms end_ms duration_ms
        start_ms="$(ms_now)"
        virsh restore "$in" --xml "$restore_xml" >/dev/null || die "virsh restore failed"
        _wait_agent "$name" 90 || die "guest agent not responding after restore"
        if [[ -s "$fs_xml" ]]; then
            local n=0 block="" line
            while IFS= read -r line || [[ -n "$line" ]]; do
                block+="$line"$'\n'
                if [[ "$line" == *"</filesystem>"* ]]; then
                    virsh attach-device "$name" <(printf '%s' "$block") --live >/dev/null \
                        || die "failed to attach restored virtiofs device"
                    n=$((n+1)); block=""
                fi
            done < "$fs_xml"
            _guest_remount_virtiofs "$name"
            ok "re-attached $n virtiofs device(s)"
        fi
        _trigger_restore_bootstrap "$name"
        # `virsh restore --xml` may produce a transient renamed domain. Define
        # its live XML (including the reattached virtiofs devices) so normal
        # management start/stop/restart semantics survive the next shutdown.
        virsh dumpxml "$name" > "$persistent_xml"
        chmod 600 "$persistent_xml"
        virsh define "$persistent_xml" >/dev/null \
            || die "restored domain could not be made persistent"
        # libvirt may briefly report the live restored domain's pre-define
        # transient state while committing the persistent definition. Require
        # several consecutive positive observations so the handoff never
        # returns during that transition.
        _wait_domain_persistent "$name" \
            || die "restored domain remained non-persistent after define"
        end_ms="$(ms_now)"
        duration_ms=$((end_ms - start_ms))
        (( duration_ms <= LIBVIRT_RESTORE_LATENCY_BUDGET_MS )) \
            || die "restore latency ${duration_ms}ms exceeded budget ${LIBVIRT_RESTORE_LATENCY_BUDGET_MS}ms"
        local enrollment_ready=false ready_duration_ms=0
        if [[ "$wait_enrollment_ready" == true ]]; then
            [[ "$RESTORE_BOOTSTRAP_ISSUED" == true ]] || die "enrollment readiness requested without bootstrap material"
            _wait_for_enrollment_ready "$inbox" "$RESTORE_BOOTSTRAP_SPIFFE" "$start_ms"
            enrollment_ready=true
            ready_duration_ms="$RESTORE_READY_DURATION_MS"
        fi
        jq --arg spiffe "$RESTORE_BOOTSTRAP_SPIFFE" \
            --argjson issued "$RESTORE_BOOTSTRAP_ISSUED" \
            '. + {bootstrap_token_issued:$issued,bootstrap_spiffe_id:$spiffe}' \
            "$vm_info" > "$vm_dir/enroll-on-restore.json"
        cp -f -- "$persistent_xml" "$domain_xml"
        chmod 600 "$domain_xml"
        [[ -z "$vm_info_backup" ]] || rm -f -- "$vm_info_backup"
        [[ -z "$domain_xml_backup" ]] || rm -f -- "$domain_xml_backup"
        restore_succeeded=true
        trap - EXIT
        printf '{"name":"%s","checkpoint_id":"%s","instance_id":"%s","vsock_cid":%s,"ip":"%s","duration_ms":%s,"enroll_on_restore":true,"bootstrap_token_issued":%s,"bootstrap_spiffe_id":"%s","enrollment_ready":%s,"ready_duration_ms":%s}\n' \
            "$(json_escape "$name")" "$(json_escape "$checkpoint_id")" "$(json_escape "$instance_id")" \
            "$cid" "$(json_escape "$ip")" "$duration_ms" "$RESTORE_BOOTSTRAP_ISSUED" \
            "$(json_escape "$RESTORE_BOOTSTRAP_SPIFFE")" "$enrollment_ready" "$ready_duration_ms"
        return 0
    fi

    # If we have an NVRAM sidecar and can determine its target path, place it first
    # (needed for cross-host / cold restore; harmless same-host).
    if [ -r "$in.nvram" ]; then
        local tgt; tgt="$(virsh save-image-dumpxml "$in" 2>/dev/null | sed -n "s/.*<nvram[^>]*>\(.*\)<\/nvram>.*/\1/p" | head -1)"
        if [ -n "$tgt" ] && [ ! -e "$tgt" ]; then cp -f "$in.nvram" "$tgt" && log "placed NVRAM at $tgt"; fi
    fi

    local t0 t1; t0=$(date +%s.%N)
    virsh restore "$in" >/dev/null || die "virsh restore failed"
    # domain name from the saved image if not provided
    [ -n "$name" ] || name="$(virsh save-image-dumpxml "$in" 2>/dev/null | sed -n 's:.*<name>\(.*\)</name>.*:\1:p' | head -1)"
    [ -n "$name" ] || die "could not determine restored domain name; pass --name"
    _wait_agent "$name" 90 || warn "guest agent not responding after restore"
    t1=$(date +%s.%N)
    ok "restored '$name' -> usable in $(awk "BEGIN{printf \"%.2f\",$t1-$t0}")s"

    # re-attach virtiofs + remount
    if [ -r "$in.virtiofs.xml" ] && [ -s "$in.virtiofs.xml" ]; then
        local n=0 blk="" line
        while IFS= read -r line || [ -n "$line" ]; do
            blk+="$line"$'\n'
            if printf '%s' "$line" | grep -q '</filesystem>'; then
                if virsh attach-device "$name" <(printf '%s' "$blk") --live >/dev/null 2>&1; then n=$((n+1)); else warn "failed to re-attach a virtiofs device"; fi
                blk=""
            fi
        done < "$in.virtiofs.xml"
        ok "re-attached $n virtiofs device(s)"
        _guest_remount_virtiofs "$name"
    fi
}

# --- named checkpoints + warm pools ----------------------------------------
cmd_checkpoint() {
    local vm="" checkpoint_id="" pre_enrollment=false managed=false
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --vm) vm="$2"; shift 2 ;;
            --id) checkpoint_id="$2"; shift 2 ;;
            --pre-enrollment) pre_enrollment=true; shift ;;
            --managed) managed=true; shift ;;
            *) die "unknown checkpoint argument: $1" ;;
        esac
    done
    [[ -n "$vm" && -n "$checkpoint_id" ]] || die "checkpoint requires --vm and --id"
    validate_identifier "VM name" "$vm"
    validate_identifier "checkpoint id" "$checkpoint_id"
    [[ "$pre_enrollment" == true ]] \
        || die "management checkpoints must be explicitly pre-enrollment"
    local out
    out="$(checkpoint_path_for "$checkpoint_id")"
    exec 8>"$(checkpoint_registry_lock)"
    flock -w 30 8 || die "timed out acquiring checkpoint registry lock"
    [[ ! -e "$(dirname "$out")" ]] || die "checkpoint already exists: $checkpoint_id"
    local -a args=(--id "$checkpoint_id" --pre-enrollment)
    [[ "$managed" == true ]] && args+=(--managed)
    cmd_save "$vm" "$out" "${args[@]}"
    # A libvirt save image retains its original UUID. The clean source domain
    # is therefore consumed by checkpoint creation and must be undefined before
    # the saved identity can be restored for a fresh tenant instance.
    virsh undefine "$vm" --nvram >/dev/null \
        || die "checkpoint saved but source domain could not be undefined"
    remove_cid_allocation "$vm" \
        || die "checkpoint saved but source vsock CID could not be released"
    local state_tmp
    state_tmp="$(mktemp "$(dirname "$out")/.state.XXXXXX")"
    jq -n --arg checkpoint_id "$checkpoint_id" --arg source_vm "$vm" \
        '{schema:1,backend:"libvirt",checkpoint_id:$checkpoint_id,source_vm:$source_vm,status:"available"}' \
        > "$state_tmp"
    chmod 600 "$state_tmp"
    mv -fT -- "$state_tmp" "$(checkpoint_state_for "$out")"
    flock -u 8
    exec 8>&-
}

cmd_restore_checkpoint() {
    local checkpoint_id="" name="" instance_id="" wait_enrollment_ready=false
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --checkpoint) checkpoint_id="$2"; shift 2 ;;
            --name) name="$2"; shift 2 ;;
            --instance-id) instance_id="$2"; shift 2 ;;
            --wait-enrollment-ready) wait_enrollment_ready=true; shift ;;
            *) die "unknown restore-checkpoint argument: $1" ;;
        esac
    done
    [[ -n "$checkpoint_id" && -n "$name" && -n "$instance_id" ]] \
        || die "restore-checkpoint requires --checkpoint, --name, and --instance-id"
    local in
    in="$(checkpoint_path_for "$checkpoint_id")"
    local state
    state="$(checkpoint_state_for "$in")"
    exec 8>"$(checkpoint_registry_lock)"
    flock -w 30 8 || die "timed out acquiring checkpoint registry lock"
    [[ -r "$state" && "$(jq -r '.status' "$state")" == "available" ]] \
        || die "checkpoint is unavailable or already reserved/consumed: $checkpoint_id"
    local -a args=(--name "$name" --instance-id "$instance_id" --checkpoint-id "$checkpoint_id")
    [[ "$wait_enrollment_ready" == true ]] && args+=(--wait-enrollment-ready)
    local output result_json state_tmp
    output="$(LIBVIRT_INTERNAL_RESTORE=1 cmd_restore "$in" "${args[@]}")"
    result_json="$(printf '%s\n' "$output" | tail -n1)"
    printf '%s' "$result_json" | jq -e . >/dev/null || die "checkpoint restore returned invalid JSON"
    state_tmp="$(mktemp "$(dirname "$in")/.state.XXXXXX")"
    jq --arg name "$name" --arg instance_id "$instance_id" \
        '.status="consumed" | .handed_to=$name | .instance_id=$instance_id' "$state" > "$state_tmp"
    chmod 600 "$state_tmp"
    mv -fT -- "$state_tmp" "$state"
    flock -u 8
    exec 8>&-
    printf '%s\n' "$output"
}

cmd_warm_init() {
    local pool=""
    local -a checkpoint_ids=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --checkpoint) checkpoint_ids+=("$2"); shift 2 ;;
            --pool) pool="$2"; shift 2 ;;
            *) die "unknown warm-init argument: $1" ;;
        esac
    done
    [[ -n "$pool" && "${#checkpoint_ids[@]}" -gt 0 ]] \
        || die "warm-init requires --pool and one or more --checkpoint IDs"
    (( ${#checkpoint_ids[@]} <= 64 )) || die "warm pool size must be <= 64"
    local pool_dir
    pool_dir="$(pool_dir_for "$pool")"
    exec 8>"$(checkpoint_registry_lock)"
    flock -w 30 8 || die "timed out acquiring checkpoint registry lock"
    [[ ! -e "$pool_dir" ]] || die "warm pool already exists: $pool"
    local slots='[]' i=0 checkpoint_id source source_state
    local -A seen=()
    for checkpoint_id in "${checkpoint_ids[@]}"; do
        validate_identifier "checkpoint id" "$checkpoint_id"
        [[ -z "${seen[$checkpoint_id]:-}" ]] || die "duplicate warm-pool checkpoint: $checkpoint_id"
        seen[$checkpoint_id]=1
        source="$(checkpoint_path_for "$checkpoint_id")"
        source_state="$(checkpoint_state_for "$source")"
        [[ -r "$source.metadata.json" && -r "$source_state" ]] || die "checkpoint not found: $checkpoint_id"
        jq -e '.pre_enrollment == true' "$source.metadata.json" >/dev/null \
            || die "warm pools require pre-enrollment checkpoints"
        [[ "$(jq -r '.status' "$source_state")" == "available" ]] \
            || die "checkpoint is already reserved or consumed: $checkpoint_id"
        i=$((i + 1))
        slots="$(printf '%s' "$slots" | jq --arg slot "slot-$i" --arg checkpoint_id "$checkpoint_id" --arg path "$source" \
            '. + [{id:$slot,status:"available",checkpoint_id:$checkpoint_id,checkpoint_path:$path}]')"
    done
    mkdir -p "$pool_dir"
    chmod 700 "$pool_dir"
    local state_tmp
    state_tmp="$(mktemp "$pool_dir/.state.XXXXXX")"
    jq -n --arg pool "$pool" --argjson size "${#checkpoint_ids[@]}" --argjson slots "$slots" \
        '{schema:1,backend:"libvirt",pool:$pool,size:$size,slots:$slots}' > "$state_tmp"
    chmod 600 "$state_tmp"
    mv -fT -- "$state_tmp" "$pool_dir/state.json"
    for checkpoint_id in "${checkpoint_ids[@]}"; do
        source="$(checkpoint_path_for "$checkpoint_id")"
        source_state="$(checkpoint_state_for "$source")"
        state_tmp="$(mktemp "$(dirname "$source")/.state.XXXXXX")"
        jq --arg pool "$pool" '.status="reserved" | .pool=$pool' "$source_state" > "$state_tmp"
        chmod 600 "$state_tmp"
        mv -fT -- "$state_tmp" "$source_state"
    done
    flock -u 8
    exec 8>&-
    printf '{"pool":"%s","size":%s,"available":%s,"prebooted":true}\n' \
        "$(json_escape "$pool")" "${#checkpoint_ids[@]}" "${#checkpoint_ids[@]}"
}

cmd_warm_handoff() {
    local pool="" instance_id="" wait_enrollment_ready=false
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --pool) pool="$2"; shift 2 ;;
            --instance-id) instance_id="$2"; shift 2 ;;
            --wait-enrollment-ready) wait_enrollment_ready=true; shift ;;
            *) die "unknown warm-handoff argument: $1" ;;
        esac
    done
    [[ -n "$pool" && -n "$instance_id" ]] \
        || die "warm-handoff requires --pool and --instance-id"
    local pool_dir state lock slot_id slot_path checkpoint_id checkpoint_state output result_json tmp
    pool_dir="$(pool_dir_for "$pool")"
    state="$pool_dir/state.json"
    lock="$pool_dir/handoff.lock"
    [[ -r "$state" ]] || die "warm pool not found: $pool"
    exec 8>"$(checkpoint_registry_lock)"
    flock -w 30 8 || die "timed out acquiring checkpoint registry lock"
    exec 9>"$lock"
    flock -w 30 9 || die "timed out acquiring warm-pool lock"
    slot_id="$(jq -r '.slots[] | select(.status == "available") | .id' "$state" | head -n1)"
    [[ -n "$slot_id" ]] || die "warm pool exhausted: $pool"
    slot_path="$(jq -r --arg slot "$slot_id" '.slots[] | select(.id == $slot) | .checkpoint_path' "$state")"
    checkpoint_id="$(jq -r --arg slot "$slot_id" '.slots[] | select(.id == $slot) | .checkpoint_id' "$state")"
    local name
    name="$(jq -r '.source_vm // empty' "$slot_path.metadata.json")"
    validate_identifier "warm-slot domain name" "$name"
    checkpoint_state="$(checkpoint_state_for "$slot_path")"
    [[ -r "$checkpoint_state" \
        && "$(jq -r '.status' "$checkpoint_state")" == "reserved" \
        && "$(jq -r '.pool' "$checkpoint_state")" == "$pool" ]] \
        || die "warm slot checkpoint reservation is invalid: $checkpoint_id"
    local -a args=(--name "$name" --instance-id "$instance_id" --checkpoint-id "$checkpoint_id")
    [[ "$wait_enrollment_ready" == true ]] && args+=(--wait-enrollment-ready)
    output="$(LIBVIRT_INTERNAL_RESTORE=1 cmd_restore "$slot_path" "${args[@]}")"
    result_json="$(printf '%s\n' "$output" | tail -n1)"
    printf '%s' "$result_json" | jq -e . >/dev/null || die "warm handoff restore returned invalid JSON"
    tmp="$(mktemp "$pool_dir/.state.XXXXXX")"
    jq --arg slot "$slot_id" --arg name "$name" --arg instance_id "$instance_id" \
        '(.slots[] | select(.id == $slot)) |= (.status="consumed" | .handed_to=$name | .instance_id=$instance_id)' \
        "$state" > "$tmp"
    chmod 600 "$tmp"
    mv -fT -- "$tmp" "$state"
    tmp="$(mktemp "$(dirname "$slot_path")/.state.XXXXXX")"
    jq --arg name "$name" --arg instance_id "$instance_id" \
        '.status="consumed" | .handed_to=$name | .instance_id=$instance_id' "$checkpoint_state" > "$tmp"
    chmod 600 "$tmp"
    mv -fT -- "$tmp" "$checkpoint_state"
    flock -u 9
    exec 9>&-
    flock -u 8
    exec 8>&-
    printf '%s' "$result_json" | jq --arg pool "$pool" --arg slot "$slot_id" \
        '. + {pool:$pool,claimed_slot:$slot}'
}

# --- selftest --------------------------------------------------------------
cmd_selftest() {
    local latency_budget_ms="$LIBVIRT_RESTORE_LATENCY_BUDGET_MS"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --latency-budget-ms) latency_budget_ms="$2"; shift 2 ;;
            *) die "unknown selftest argument: $1" ;;
        esac
    done
    [[ "$latency_budget_ms" =~ ^[0-9]+$ && "$latency_budget_ms" -gt 0 ]] \
        || die "latency budget must be a positive integer"
    local BASE_VM=chkpt-selftest-base CID=""
    local SOURCE_INSTANCE_ID=018fc0a2-6430-7aaa-bbbb-ccccdddd0000
    local WORK=/var/tmp/chkpt-selftest
    local HOST_CID_REGISTRY="$CID_REGISTRY" HOST_IP_REGISTRY="$IP_REGISTRY"
    VM_STORAGE_DIR="$WORK/vms"
    AGENTSHARE_ROOT="$WORK/agentshare"
    IP_REGISTRY="$WORK/ip-registry"
    CID_REGISTRY="$WORK/cid-registry"
    LIBVIRT_CHECKPOINT_ROOT="$WORK/checkpoints"
    LIBVIRT_WARM_POOL_ROOT="$WORK/warm-pools"
    _selftest_cleanup() {
        local child_info="$VM_STORAGE_DIR/chkpt-selftest-base/vm-info.json"
        if [[ -r "$child_info" ]]; then
            local child_ip child_mac
            child_ip="$(jq -r '.ip // empty' "$child_info")"
            child_mac="$(jq -r '.mac // empty' "$child_info")"
            [[ -z "$child_ip" || -z "$child_mac" ]] \
                || remove_dhcp_reservation "${LIBVIRT_NETWORK:-default}" chkpt-selftest-base "$child_mac" "$child_ip" || true
        fi
        remove_cid_allocation chkpt-selftest-base || true
        local domain
        for domain in chkpt-selftest-base chkpt-selftest; do
            virsh destroy "$domain" >/dev/null 2>&1 || true
            virsh undefine "$domain" --nvram >/dev/null 2>&1 || true
        done
        rm -rf -- /var/tmp/chkpt-selftest
    }
    trap _selftest_cleanup EXIT
    local BASE="${AIWG_BASE_IMAGE:-${BASE_IMAGES_DIR}/ubuntu-server-24.04-agent.qcow2}"
    local CODE=/usr/share/OVMF/OVMF_CODE_4M.fd VARS=/usr/share/OVMF/OVMF_VARS_4M.fd
    command -v virsh >/dev/null || die "virsh not found"
    [ -r "$BASE" ] || die "base image not found: $BASE"
    log "selftest: building throwaway clean base $BASE_VM"
    _selftest_cleanup
    local SOURCE_VM_DIR="$VM_STORAGE_DIR/$BASE_VM"
    mkdir -p "$WORK/global-ro" "$WORK/inbox" "$SOURCE_VM_DIR" "$AGENTSHARE_ROOT/$BASE_VM-inbox"
    chmod -R 0777 "$WORK"
    # The selftest owns disposable registries, but it runs before the E2E
    # resource VM is reaped. Seed its allocation view from the host so neither
    # the source VM nor its fresh child can reuse a live VM's IP or vsock CID.
    : > "$CID_REGISTRY"
    : > "$IP_REGISTRY"
    [[ ! -r "$HOST_CID_REGISTRY" ]] || cp -- "$HOST_CID_REGISTRY" "$CID_REGISTRY"
    [[ ! -r "$HOST_IP_REGISTRY" ]] || cp -- "$HOST_IP_REGISTRY" "$IP_REGISTRY"
    CID_START=6390
    CID_END=6399
    CID="$(allocate_cid_for_vm "$BASE_VM" "$SOURCE_INSTANCE_ID")" \
        || die "failed to allocate selftest source vsock CID"
    allocate_ip_for_vm "$BASE_VM" "${LIBVIRT_NETWORK:-default}" >/dev/null \
        || die "failed to reserve selftest source IP"
    local MARK="hello-643-$$"; echo "$MARK" > "$WORK/global-ro/marker"
    qemu-img create -f qcow2 -F qcow2 -b "$BASE" "$SOURCE_VM_DIR/$BASE_VM.qcow2" >/dev/null
    chmod 0666 "$SOURCE_VM_DIR/$BASE_VM.qcow2"
    cp "$VARS" "$SOURCE_VM_DIR/${BASE_VM}_VARS.fd"; chmod 0666 "$SOURCE_VM_DIR/${BASE_VM}_VARS.fd"
    jq -n --arg instance_id "$SOURCE_INSTANCE_ID" --arg name "$BASE_VM" --argjson cid "$CID" \
        '{instance_id:$instance_id,name:$name,vsock_cid:$cid}' > "$SOURCE_VM_DIR/vm-info.json"
    cat > "$WORK/domain.xml" <<XML
<domain type='kvm'>
  <name>$BASE_VM</name><memory unit='MiB'>2048</memory><vcpu>2</vcpu>
  <memoryBacking><source type='memfd'/><access mode='shared'/></memoryBacking>
  <os><type arch='x86_64' machine='q35'>hvm</type>
    <loader readonly='yes' type='pflash'>$CODE</loader><nvram>$SOURCE_VM_DIR/${BASE_VM}_VARS.fd</nvram><boot dev='hd'/></os>
  <features><acpi/><apic/></features><cpu mode='host-passthrough'/>
  <devices><emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type='file' device='disk'><driver name='qemu' type='qcow2' cache='writeback'/><source file='$SOURCE_VM_DIR/$BASE_VM.qcow2'/><target dev='vda' bus='virtio'/></disk>
    <interface type='network'><source network='default'/><model type='virtio'/></interface>
    <filesystem type='mount' accessmode='passthrough'><driver type='virtiofs'/><source dir='$WORK/global-ro'/><target dir='agentglobal'/></filesystem>
    <filesystem type='mount' accessmode='passthrough'><driver type='virtiofs'/><source dir='$WORK/inbox'/><target dir='agentinbox'/></filesystem>
    <vsock model='virtio'><cid auto='no' address='$CID'/></vsock>
    <channel type='unix'><target type='virtio' name='org.qemu.guest_agent.0'/></channel>
    <serial type='pty'><target port='0'/></serial><console type='pty'><target type='serial' port='0'/></console>
  </devices><on_poweroff>destroy</on_poweroff><on_reboot>restart</on_reboot><on_crash>destroy</on_crash>
</domain>
XML
    virsh define "$WORK/domain.xml" >/dev/null
    virsh start "$BASE_VM" >/dev/null
    _wait_agent "$BASE_VM" 120 || die "guest agent never came up"
    ok "VM booted, guest agent up"
    # Mount both shares so the checkpoint exercises mandatory live detach.
    virsh qemu-agent-command "$BASE_VM" '{"execute":"guest-exec","arguments":{"path":"/bin/sh","arg":["-c","mkdir -p /mnt/global /mnt/inbox; mountpoint -q /mnt/global || mount -t virtiofs agentglobal /mnt/global; mountpoint -q /mnt/inbox || mount -t virtiofs agentinbox /mnt/inbox"]}}' >/dev/null 2>&1
    sleep 2

    local CKPT=$LIBVIRT_CHECKPOINT_ROOT/selftest/checkpoint.save
    log "clean checkpoint..."; cmd_checkpoint --vm "$BASE_VM" --id selftest --pre-enrollment
    [ -s "$CKPT" ] || die "checkpoint image empty"
    [ -s "$CKPT.virtiofs.xml" ] || die "virtiofs sidecar missing/empty"
    grep -q 'virtiofs' "$CKPT.virtiofs.xml" || die "virtiofs sidecar has no filesystem block"
    [ -s "$CKPT.nvram" ] || die "NVRAM sidecar missing"
    jq -e '.pre_enrollment == true and .source_vm == "chkpt-selftest-base"' "$CKPT.metadata.json" >/dev/null \
        || die "clean checkpoint metadata missing"
    ok "checkpoint artifacts present (image + virtiofs.xml + nvram)"
    if virsh dominfo "$BASE_VM" >/dev/null 2>&1; then
        die "source domain still exists after consumable checkpoint creation"
    fi
    [[ -r "$SOURCE_VM_DIR/$BASE_VM.qcow2" ]] \
        || die "checkpoint unexpectedly removed the source disk backing the restore overlay"
    if grep -q "$SOURCE_INSTANCE_ID" "$CID_REGISTRY"; then
        die "checkpoint retained the stopped source instance's vsock CID allocation"
    fi

    local restore_output restore_json restore_duration_ms instance_id
    instance_id="018fc0a2-6430-7aaa-bbbb-ccccddddeeee"
    # Keep the fresh-identity range distinct from the source range. Host
    # allocations copied above are still honored within this range.
    CID_START=6400
    CID_END=65535
    LIBVIRT_RESTORE_LATENCY_BUDGET_MS="$latency_budget_ms"
    unset LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD
    log "fresh restore..."
    restore_output="$(cmd_restore_checkpoint --checkpoint selftest --name "$BASE_VM" --instance-id "$instance_id")"
    printf '%s\n' "$restore_output"
    restore_json="$(printf '%s\n' "$restore_output" | tail -n1)"
    restore_duration_ms="$(printf '%s' "$restore_json" | jq -r '.duration_ms')"
    (( restore_duration_ms <= latency_budget_ms )) \
        || die "default-profile restore latency ${restore_duration_ms}ms exceeded ${latency_budget_ms}ms budget"
    printf '%s' "$restore_json" | jq -e --arg id "$instance_id" --argjson old_cid "$CID" \
        '.instance_id == $id and .enroll_on_restore == true and (.vsock_cid > 2) and (.vsock_cid != $old_cid)' >/dev/null \
        || die "fresh restore result did not record new identity/CID"
    _agent_ok "$BASE_VM" || die "guest agent not responding after restore"
    virsh dumpxml "$BASE_VM" | grep -q 'virtiofs' || die "virtiofs not re-attached after restore"
    _wait_domain_persistent "$BASE_VM" \
        || die "freshly restored domain is not persistent"
    local seen
    seen="$(_guest_exec_capture "$BASE_VM" 'cat /mnt/global/marker 2>/dev/null')" \
        || die "restored guest could not read the global virtiofs marker"
    [[ "$seen" == "$MARK" ]] || die "restored guest read unexpected global marker"
    ok "fresh child identity, persistence, guest agent, and virtiofs verified"

    log "cleanup..."
    _selftest_cleanup
    trap - EXIT
    echo -e "${GREEN}SELFTEST PASSED${NC}: clean checkpoint -> fresh persistent restore with virtiofs works."
    printf '{"backend":"libvirt","profile":"q35-uefi-2g","restore_duration_ms":%s,"budget_ms":%s,"passed":true}\n' \
        "$restore_duration_ms" "$latency_budget_ms"
}

# --- dispatch --------------------------------------------------------------
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    case "${1:-}" in
        save)     shift; cmd_save "$@";;
        restore)  shift; cmd_restore "$@";;
        checkpoint) shift; load_bootstrap_stdin; cmd_checkpoint "$@";;
        restore-checkpoint) shift; load_bootstrap_stdin; cmd_restore_checkpoint "$@";;
        warm-init) shift; load_bootstrap_stdin; cmd_warm_init "$@";;
        warm-handoff) shift; load_bootstrap_stdin; cmd_warm_handoff "$@";;
        selftest) shift; cmd_selftest "$@";;
        -h|--help|"") usage;;
        *) usage; die "unknown subcommand '$1'";;
    esac
fi
