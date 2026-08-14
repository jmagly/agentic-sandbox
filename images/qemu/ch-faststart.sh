#!/usr/bin/env bash
# ch-faststart.sh - Cloud Hypervisor snapshot/restore/fork/warm-pool operations.
#
# This is the script boundary used by the management API for CH-5/CH-6/CH-7.
# It keeps base snapshots pre-enrollment by construction, writes provenance
# metadata, verifies base bundles before restore, allocates fresh CIDs for each
# restored child, and refuses secret-bearing snapshots unless they are sealed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

LOGGING_LIB="$PROJECT_ROOT/scripts/lib/logging.sh"
if [[ -f "$LOGGING_LIB" && "${USE_SHARED_LOGGING:-true}" == "true" ]]; then
    # shellcheck source=../../scripts/lib/logging.sh
    source "$LOGGING_LIB"
    LOG_SCRIPT_NAME="ch-faststart"
else
    log_info() { echo "[INFO] $*"; }
    log_success() { echo "[OK] $*"; }
    log_warn() { echo "[WARN] $*" >&2; }
    log_error() { echo "[ERROR] $*" >&2; }
fi

VM_STORAGE_DIR="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
IP_REGISTRY="${IP_REGISTRY:-$VM_STORAGE_DIR/.ip-registry}"
CID_REGISTRY="${CID_REGISTRY:-$VM_STORAGE_DIR/.vsock-cid-registry}"
IP_BASE="${IP_BASE:-192.168.122}"
IP_START="${IP_START:-201}"
IP_END="${IP_END:-254}"
CH_SNAPSHOT_ROOT="${CH_SNAPSHOT_ROOT:-$VM_STORAGE_DIR/.ch-snapshots}"
CH_WARM_POOL_ROOT="${CH_WARM_POOL_ROOT:-$VM_STORAGE_DIR/.ch-warm-pool}"
CH_RESTORE_LATENCY_BUDGET_MS="${CH_RESTORE_LATENCY_BUDGET_MS:-1000}"
CH_RESTORE_READY_LATENCY_BUDGET_MS="${CH_RESTORE_READY_LATENCY_BUDGET_MS:-30000}"
CH_WARM_HANDOFF_READY_LATENCY_BUDGET_MS="${CH_WARM_HANDOFF_READY_LATENCY_BUDGET_MS:-1000}"
CH_ENROLLMENT_READY_TIMEOUT_MS="${CH_ENROLLMENT_READY_TIMEOUT_MS:-30000}"
CH_WARM_READY_TIMEOUT_MS="${CH_WARM_READY_TIMEOUT_MS:-30000}"
CH_SNAPSHOT_SIGN_KEY="${CH_SNAPSHOT_SIGN_KEY:-}"
CH_SNAPSHOT_SIGNER_FINGERPRINT="${CH_SNAPSHOT_SIGNER_FINGERPRINT:-}"
CH_CLEAN_ATTESTATION_MAX_AGE_SECS="${CH_CLEAN_ATTESTATION_MAX_AGE_SECS:-300}"
AGENTIC_BACKEND="${AGENTIC_BACKEND:-cloud-hypervisor}"
export VM_STORAGE_DIR IP_REGISTRY CID_REGISTRY IP_BASE IP_START IP_END AGENTIC_BACKEND

# shellcheck source=lib/network.sh
source "$SCRIPT_DIR/lib/network.sh"
# shellcheck source=lib/platform.sh
source "$SCRIPT_DIR/lib/platform.sh"

usage() {
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
    cat <<EOF

Usage:
  $0 snapshot --vm NAME --id ID --pre-enrollment --sign-key KEY
  $0 clean-prepare --vm NAME --host HOST --user USER [--identity PATH]
  $0 snapshot --vm NAME --id ID --secret-bearing --seal-key KEY [--sign-key KEY]
  $0 restore  --snapshot ID --name NAME [--mode ondemand|copy] [--instance-id ID] [--wait-enrollment-ready]
  $0 fork     --snapshot ID --prefix NAME --count N [--mode ondemand|copy] [--paused] [--wait-enrollment-ready]
  $0 fork-evidence --prefix NAME --count N [--manifest PATH]
  $0 warm-init --snapshot ID --size N --prefix NAME
  $0 warm-handoff --pool NAME --name NAME [--mode ondemand|copy] [--wait-enrollment-ready]
  $0 verify   --snapshot ID
EOF
}

die() {
    log_error "$*"
    exit 1
}

SNAPSHOT_CLEANUP_DIR=""
CH_CLEANUP_CHILDREN=()
cleanup_partial_snapshot() {
    local dir="${SNAPSHOT_CLEANUP_DIR:-}"
    SNAPSHOT_CLEANUP_DIR=""
    [[ -n "$dir" ]] || return 0
    local id="${dir#"$CH_SNAPSHOT_ROOT"/}"
    if [[ "$dir" != "$CH_SNAPSHOT_ROOT/$id" || ! "$id" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
        log_error "refusing to clean invalid snapshot path: $dir"
        return 1
    fi
    rm -rf -- "$dir"
}

cleanup_partial_children() {
    local child
    for child in "${CH_CLEANUP_CHILDREN[@]:-}"; do
        [[ -n "$child" ]] || continue
        backend_destroy_vm "$child" >/dev/null 2>&1 || true
    done
    CH_CLEANUP_CHILDREN=()
}

validate_identifier() {
    local label="$1" value="$2"
    if [[ ! "$value" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
        die "$label must match [A-Za-z0-9][A-Za-z0-9._-]{0,127}"
    fi
}

json_escape() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    value="${value//$'\n'/\\n}"
    printf '%s' "$value"
}

ms_now() {
    # Restore and handoff budgets measure elapsed time. Runner wall-clock
    # corrections must not turn a sub-second operation into a multi-day one.
    awk '{printf "%.0f\n", $1 * 1000}' /proc/uptime
}

bootstrap_json_field() {
    local json="$1" field="$2"
    if [[ -z "$json" || "$json" == "null" ]]; then
        return 0
    fi
    command -v jq >/dev/null 2>&1 || die "jq is required for CH bootstrap enrollment metadata"
    printf '%s' "$json" | jq -r --arg field "$field" '.[$field] // empty'
}

bootstrap_json_for_child() {
    local name="$1"
    if [[ -n "${CH_BOOTSTRAP_STDIN_PAYLOAD:-}" ]]; then
        command -v jq >/dev/null 2>&1 || die "jq is required for CH child bootstrap enrollment metadata"
        printf '%s' "$CH_BOOTSTRAP_STDIN_PAYLOAD" | jq -c --arg name "$name" '.children[$name] // empty'
        return 0
    fi
    if [[ -z "${CH_CHILD_BOOTSTRAP_ENVELOPES:-}" ]]; then
        return 0
    fi
    command -v jq >/dev/null 2>&1 || die "jq is required for CH child bootstrap enrollment metadata"
    printf '%s' "$CH_CHILD_BOOTSTRAP_ENVELOPES" | jq -c --arg name "$name" '.[$name] // empty'
}

bootstrap_json_for_single_child() {
    if [[ -z "${CH_BOOTSTRAP_STDIN_PAYLOAD:-}" ]]; then
        return 0
    fi
    command -v jq >/dev/null 2>&1 || die "jq is required for CH bootstrap enrollment metadata"
    printf '%s' "$CH_BOOTSTRAP_STDIN_PAYLOAD" | jq -c '.single // empty'
}

load_bootstrap_stdin() {
    CH_BOOTSTRAP_STDIN_PAYLOAD=""
    if [[ "${CH_BOOTSTRAP_STDIN:-}" != "1" ]]; then
        return 0
    fi
    command -v jq >/dev/null 2>&1 || die "jq is required for CH bootstrap stdin"
    IFS= read -r CH_BOOTSTRAP_STDIN_PAYLOAD || die "CH bootstrap stdin was empty"
    printf '%s' "$CH_BOOTSTRAP_STDIN_PAYLOAD" | jq -e 'type == "object"' >/dev/null \
        || die "CH bootstrap stdin must be a JSON object"
}

bootstrap_input_present() {
    [[ -n "${CH_BOOTSTRAP_STDIN_PAYLOAD:-}" \
        || -n "${CH_CHILD_BOOTSTRAP_ENVELOPES:-}" \
        || -n "${AGENT_BOOTSTRAP_TOKEN:-}" \
        || -n "${AGENT_BOOTSTRAP_SPIFFE_ID:-}" ]]
}

require_snapshot_agentshare_for_bootstrap() {
    local snapshot_dir="$1"
    bootstrap_input_present || return 0
    local source_state="$snapshot_dir/source-vm.env"
    if ! grep -q '^USE_AGENTSHARE=true$' "$source_state" 2>/dev/null \
        || ! grep -q '^INBOX_PATH=.\+' "$source_state" 2>/dev/null; then
        die "bootstrap enrollment requires an agentshare-enabled snapshot with an isolated child inbox"
    fi
}

child_state_field() {
    local name="$1" field="$2"
    local state_file="$VM_STORAGE_DIR/$name/cloud-hypervisor/vm.env"
    [[ -f "$state_file" ]] || return 0
    sed -n "s/^${field}=//p" "$state_file" | head -n1
}

write_restore_bootstrap_env() {
    local path="$1" instance_id="$2" token="$3" spiffe="$4" expires="$5" tls_dir="$6" enrollment_url="$7" ca_pem="$8" mac="$9" ip="${10}"
    local dir tmp
    dir="$(dirname "$path")"
    mkdir -p "$dir"
    tmp="$(mktemp "$dir/.restore-bootstrap.env.XXXXXX")"
    if ! (
        umask 077
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
EOF
        if [[ -n "$expires" ]]; then
            printf 'AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=%s\n' "$expires" >> "$tmp"
        fi
        if [[ -n "$tls_dir" ]]; then
            printf 'AGENT_BOOTSTRAP_TLS_DIR=%s\n' "$tls_dir" >> "$tmp"
        fi
        if [[ -n "$enrollment_url" ]]; then
            printf 'AGENT_BOOTSTRAP_ENROLLMENT_URL=%s\n' "$enrollment_url" >> "$tmp"
        fi
        if [[ -n "$ca_pem" ]]; then
            local ca_path="$dir/restore-bootstrap-ca.pem"
            printf '%s\n' "$ca_pem" > "$ca_path"
            chmod 600 "$ca_path"
            printf 'AGENT_BOOTSTRAP_CA=/mnt/inbox/restore-bootstrap-ca.pem\n' >> "$tmp"
        fi
        chmod 600 "$tmp"
    ); then
        rm -f "$tmp"
        return 1
    fi
    if ! mv -fT -- "$tmp" "$path"; then
        rm -f "$tmp"
        return 1
    fi
    rm -f -- "$dir/restore-enrollment-ready.json"
}

write_enroll_on_restore_metadata() {
    local name="$1" instance_id="$2" snapshot="$3" cid="$4"
    local vm_dir="$VM_STORAGE_DIR/$name"
    local bootstrap_json token spiffe expires tls_dir enrollment_url ca_pem guest_bootstrap_env_path guest_bootstrap_env_mode token_issued=false
    mkdir -p "$vm_dir"

    bootstrap_json="$(bootstrap_json_for_child "$name")"
    token="${AGENT_BOOTSTRAP_TOKEN:-}"
    spiffe="${AGENT_BOOTSTRAP_SPIFFE_ID:-}"
    expires="${AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS:-}"
    tls_dir="${AGENT_BOOTSTRAP_TLS_DIR:-}"
    enrollment_url="${AGENT_BOOTSTRAP_ENROLLMENT_URL:-}"
    ca_pem="${AGENT_BOOTSTRAP_CA_PEM:-}"
    if [[ -n "${AGENT_BOOTSTRAP_INSTANCE_ID:-}" ]]; then
        instance_id="$AGENT_BOOTSTRAP_INSTANCE_ID"
    fi
    if [[ -n "$bootstrap_json" ]]; then
        local json_instance_id
        json_instance_id="$(bootstrap_json_field "$bootstrap_json" instance_id)"
        if [[ -n "$json_instance_id" ]]; then
            instance_id="$json_instance_id"
        fi
        token="$(bootstrap_json_field "$bootstrap_json" token)"
        spiffe="$(bootstrap_json_field "$bootstrap_json" spiffe_id)"
        expires="$(bootstrap_json_field "$bootstrap_json" expires_at_unix_ms)"
        tls_dir="$(bootstrap_json_field "$bootstrap_json" tls_dir)"
        enrollment_url="$(bootstrap_json_field "$bootstrap_json" enrollment_url)"
        ca_pem="$(bootstrap_json_field "$bootstrap_json" ca_pem)"
    elif [[ -n "${CH_BOOTSTRAP_STDIN_PAYLOAD:-}" ]]; then
        bootstrap_json="$(bootstrap_json_for_single_child)"
        if [[ -n "$bootstrap_json" ]]; then
            local json_instance_id
            json_instance_id="$(bootstrap_json_field "$bootstrap_json" instance_id)"
            if [[ -n "$json_instance_id" ]]; then
                instance_id="$json_instance_id"
            fi
            token="$(bootstrap_json_field "$bootstrap_json" token)"
            spiffe="$(bootstrap_json_field "$bootstrap_json" spiffe_id)"
            expires="$(bootstrap_json_field "$bootstrap_json" expires_at_unix_ms)"
            tls_dir="$(bootstrap_json_field "$bootstrap_json" tls_dir)"
            enrollment_url="$(bootstrap_json_field "$bootstrap_json" enrollment_url)"
            ca_pem="$(bootstrap_json_field "$bootstrap_json" ca_pem)"
        fi
    fi
    if [[ -n "$token" || -n "$spiffe" ]]; then
        [[ -n "$token" && -n "$spiffe" ]] || die "bootstrap enrollment requires both token and spiffe_id for $name"
        local child_inbox_path
        child_inbox_path="$(child_state_field "$name" "INBOX_PATH")"
        [[ -n "$child_inbox_path" ]] \
            || die "bootstrap enrollment requires an isolated child inbox for $name"
        guest_bootstrap_env_path="$child_inbox_path/restore-bootstrap.env"
        [[ "$enrollment_url" == https://* ]] || die "bootstrap enrollment URL must use HTTPS for $name"
        [[ -n "$ca_pem" ]] || die "bootstrap enrollment requires a host-delivered CA for $name"
        write_restore_bootstrap_env "$guest_bootstrap_env_path" "$instance_id" "$token" "$spiffe" "$expires" "$tls_dir" "$enrollment_url" "$ca_pem" \
            "$(child_state_field "$name" MAC_ADDRESS)" "$(child_state_field "$name" IP_ADDRESS)"
        guest_bootstrap_env_mode="600"
        token_issued=true
    fi
    ENROLL_BOOTSTRAP_ISSUED="$token_issued"
    ENROLL_BOOTSTRAP_SPIFFE="$spiffe"

    cat > "$vm_dir/enroll-on-restore.json" <<EOF
{
  "instance_id": "$(json_escape "$instance_id")",
  "vm_name": "$(json_escape "$name")",
  "source_snapshot": "$(json_escape "$snapshot")",
  "fresh_vsock_cid": $cid,
  "fresh_enrollment_required": true,
  "fresh_mtls_identity_required": true,
  "bootstrap_token_issued": $token_issued,
  "bootstrap_spiffe_id": "$(json_escape "$spiffe")",
  "bootstrap_token_expires_at_unix_ms": ${expires:-null},
  "bootstrap_enrollment_url": "$(json_escape "$enrollment_url")",
  "bootstrap_delivery": "$(if [[ -n "${guest_bootstrap_env_path:-}" ]]; then printf 'agentshare-inbox'; else printf 'none'; fi)",
  "guest_bootstrap_env_path": "$(json_escape "${guest_bootstrap_env_path:-}")",
  "guest_bootstrap_env_mount_path": "$(if [[ -n "${guest_bootstrap_env_path:-}" ]]; then printf '/mnt/inbox/restore-bootstrap.env'; fi)",
  "guest_bootstrap_env_mode": "${guest_bootstrap_env_mode:-}"
}

EOF
}

# Called by the Cloud Hypervisor backend after the child agentshare directories
# and state file exist, but before the restored VMM is launched. Staging the
# bootstrap drop here removes the race where a resumed agent could execute
# before its one-time enrollment material was visible.
ch_restore_prelaunch_hook() {
    local name="$1"
    local _child_inbox_path="$2"
    local snapshot="${CH_ACTIVE_RESTORE_SNAPSHOT:-}"
    local instance_id="${CH_ACTIVE_RESTORE_INSTANCE_ID:-$name}"
    [[ -n "$snapshot" ]] || die "restore prelaunch hook is missing snapshot context"
    if [[ "${CH_WARM_PREPARE:-false}" == "true" ]]; then
        local warm_dir warm_env warm_tmp
        warm_dir="$(dirname "$_child_inbox_path")/$(basename "$_child_inbox_path")"
        warm_env="$warm_dir/restore-bootstrap.env"
        mkdir -p "$warm_dir"
        rm -f "$warm_dir/warm-slot-ready.json" "$warm_dir/restore-enrollment-ready.json"
        warm_tmp="$(mktemp "$warm_dir/.warm-prepare.XXXXXX")"
        (
            umask 077
            printf 'AGENT_WARM_PREPARE=true\nAGENT_RESTORE_HOST_ADDRESS=%s\nAGENT_RESTORE_MAC_ADDRESS=%s\nAGENT_RESTORE_IP_ADDRESS=%s\nAGENT_RESTORE_IP_PREFIX=%s\n' \
                "${AGENTIC_VM_HOST_ADDRESS:-192.168.122.1}" \
                "$(child_state_field "$name" MAC_ADDRESS)" \
                "$(child_state_field "$name" IP_ADDRESS)" \
                "${AGENTIC_VM_RESTORE_IP_PREFIX:-24}" > "$warm_tmp"
            chmod 600 "$warm_tmp"
        )
        mv -fT "$warm_tmp" "$warm_env"
        return 0
    fi
    if [[ -n "${CH_ACTIVE_RESTORE_CHILD_PREFIX:-}" ]]; then
        instance_id="$name"
    fi
    write_enroll_on_restore_metadata "$name" "$instance_id" "$snapshot" "$(child_state_field "$name" VSOCK_CID)"
}

wait_for_warm_slot_ready() {
    local name="$1" state inbox ack deadline
    state="$VM_STORAGE_DIR/$name/cloud-hypervisor/vm.env"
    inbox="$(sed -n 's/^INBOX_PATH=//p' "$state" | head -n1)"
    [[ -n "$inbox" ]] || die "warm slot $name has no isolated inbox"
    ack="$inbox/warm-slot-ready.json"
    deadline=$(( $(ms_now) + CH_WARM_READY_TIMEOUT_MS ))
    while (( $(ms_now) <= deadline )); do
        if [[ -f "$ack" && "$(stat -c '%a' "$ack" 2>/dev/null)" == "600" ]] \
            && jq -e '.schema == 1 and .network_ready == true and .credential_free == true' "$ack" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.05
    done
    die "warm slot $name did not reach credential-free network readiness within ${CH_WARM_READY_TIMEOUT_MS}ms"
}

trigger_guest_restore_bootstrap() {
    local name="$1" snapshot="$2"
    local snapshot_dir="$CH_SNAPSHOT_ROOT/$snapshot" source_vm known_hosts key ip
    source_vm="$(sed -n 's/^VM_NAME=//p' "$snapshot_dir/source-vm.env" | head -n1)"
    known_hosts="$snapshot_dir/restore-ssh-known-hosts"
    key="${AGENTIC_CH_GUEST_SSH_KEY:-/var/lib/agentic-sandbox/secrets/ssh-keys/$source_vm}"
    [[ -s "$known_hosts" && -f "$key" ]] \
        || die "restore guest trigger requires pinned host key evidence and SSH identity for $source_vm"
    ip="$(backend_get_vm_ip "$name" 15)" \
        || die "restore guest $name did not receive its allocated IP"
    local attempt
    for attempt in $(seq 1 100); do
        if sudo -n ssh -o BatchMode=yes -o ConnectTimeout=1 \
            -o StrictHostKeyChecking=yes -o HostKeyAlias=ch-clean-base \
            -o "UserKnownHostsFile=$known_hosts" -i "$key" \
            "${AGENTIC_CH_GUEST_SSH_USER:-agent}@$ip" \
            sudo -n systemctl start agent-client-restore-bootstrap.service \
            >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.05
    done
    die "could not activate restore bootstrap in guest $name over pinned SSH"
}

pause_warm_slot() {
    local name="$1" state api
    state="$VM_STORAGE_DIR/$name/cloud-hypervisor/vm.env"
    api="$(sed -n 's/^API_SOCKET=//p' "$state" | head -n1)"
    [[ -S "$api" ]] || die "warm slot API socket missing: $api"
    "$_ch_remote_bin" --api-socket "$api" pause >/dev/null
}

resume_warm_slot() {
    local name="$1" state api
    state="$VM_STORAGE_DIR/$name/cloud-hypervisor/vm.env"
    api="$(sed -n 's/^API_SOCKET=//p' "$state" | head -n1)"
    [[ -S "$api" ]] || die "warm slot API socket missing: $api"
    "$_ch_remote_bin" --api-socket "$api" resume >/dev/null
}

rebind_warm_slot() {
    local slot="$1" name="$2" instance_id="$3"
    local old_dir new_dir
    old_dir="$VM_STORAGE_DIR/$slot"
    new_dir="$VM_STORAGE_DIR/$name"
    [[ -d "$old_dir" && ! -e "$new_dir" ]] || die "cannot claim warm slot $slot as $name"
    local state old_inbox old_outbox new_inbox new_outbox cid
    state="$old_dir/cloud-hypervisor/vm.env"
    old_inbox="$(sed -n 's/^INBOX_PATH=//p' "$state" | head -n1)"
    old_outbox="$(sed -n 's/^OUTBOX_PATH=//p' "$state" | head -n1)"
    new_inbox="${old_inbox%/*}/$name-inbox"
    new_outbox="${old_outbox%/*}/$name-outbox"
    [[ -z "$old_inbox" || ! -e "$new_inbox" ]] || die "warm handoff inbox already exists: $new_inbox"
    [[ -z "$old_outbox" || ! -e "$new_outbox" ]] || die "warm handoff outbox already exists: $new_outbox"
    if [[ -n "$old_inbox" ]]; then
        mv -- "$old_inbox" "$new_inbox" 2>/dev/null \
            || sudo -n mv -- "$old_inbox" "$new_inbox"
    fi
    if [[ -n "$old_outbox" ]]; then
        mv -- "$old_outbox" "$new_outbox" 2>/dev/null \
            || sudo -n mv -- "$old_outbox" "$new_outbox"
    fi
    mv -- "$old_dir" "$new_dir"
    state="$new_dir/cloud-hypervisor/vm.env"
    sed -i \
        -e "s|^VM_NAME=.*|VM_NAME=$name|" \
        -e "s|^INBOX_PATH=.*|INBOX_PATH=$new_inbox|" \
        -e "s|^OUTBOX_PATH=.*|OUTBOX_PATH=$new_outbox|" \
        -e "s|$old_dir|$new_dir|g" \
        "$state"
    cid="$(sed -n 's/^VSOCK_CID=//p' "$state" | head -n1)"
    if [[ -n "$cid" && -f "$CID_REGISTRY" ]]; then
        sed -i "s/^${cid}=.*/${cid}=${instance_id}/" "$CID_REGISTRY"
    fi
    if [[ -f "$IP_REGISTRY" ]]; then
        local ip_tmp
        ip_tmp="$(mktemp "${IP_REGISTRY}.tmp.XXXXXX")"
        sed "s/^${slot}=/${name}=/" "$IP_REGISTRY" > "$ip_tmp"
        mv -f "$ip_tmp" "$IP_REGISTRY"
    fi
}

wait_for_enrollment_ready() {
    local name="$1" start_ms="$2"
    local metadata="$VM_STORAGE_DIR/$name/enroll-on-restore.json"
    local drop ack expected_spiffe
    drop="$(sed -n 's/.*"guest_bootstrap_env_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$metadata" | head -n1)"
    expected_spiffe="$(sed -n 's/.*"bootstrap_spiffe_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$metadata" | head -n1)"
    [[ -n "$drop" && -n "$expected_spiffe" ]] || die "enrollment readiness requires staged bootstrap metadata for $name"
    ack="$(dirname "$drop")/restore-enrollment-ready.json"
    local deadline=$(( $(ms_now) + CH_ENROLLMENT_READY_TIMEOUT_MS ))
    while (( $(ms_now) <= deadline )); do
        if [[ ! -e "$drop" && -f "$ack" && "$(stat -c '%a' "$ack" 2>/dev/null)" == "600" ]] \
            && jq -e --arg spiffe "$expected_spiffe" '.schema == 1 and .spiffe_id == $spiffe and .tls_materialized == true and (.certificate_sha256 | test("^[0-9a-f]{64}$"))' "$ack" >/dev/null 2>&1; then
            ENROLLMENT_READY_DURATION_MS=$(( $(ms_now) - start_ms ))
            return 0
        fi
        sleep 0.05
    done
    die "restored guest $name did not provide a validated enrollment acknowledgement within ${CH_ENROLLMENT_READY_TIMEOUT_MS}ms"
}

fork_memory_sharing_json() {
    local children="$1"
    if ! command -v jq >/dev/null 2>&1; then
        printf '{"available":false,"reason":"jq unavailable"}'
        return 0
    fi
    local tmp
    tmp="$(mktemp)"
    printf '[' > "$tmp"
    local first=true child_name metrics_path
    while IFS=$'\t' read -r child_name metrics_path; do
        [[ -n "$child_name" && -n "$metrics_path" && -f "$metrics_path" ]] || continue
        if [[ "$first" == true ]]; then
            first=false
        else
            printf ',' >> "$tmp"
        fi
        jq -c '{
            name: .child_vm,
            vmm_pid,
            duration_ms,
            guest_ram: (.guest_ram // {available:false}),
            snapshot_backing: (.snapshot_backing // {available:false}),
            vmm_process: {
                rss_kb: (.vmm_rss_kb // 0),
                pss_kb: (.vmm_pss_kb // 0),
                shared_clean_kb: (.vmm_shared_clean_kb // 0),
                shared_dirty_kb: (.vmm_shared_dirty_kb // 0)
            }
        }' "$metrics_path" >> "$tmp"
    done < <(printf '%s' "$children" | jq -r '.[] | [.name, .metrics] | @tsv')
    printf ']\n' >> "$tmp"
    jq -c '
      . as $children |
      ($children | map(select(.guest_ram.available == true))) as $measured |
      ($measured | map(.guest_ram.rss_kb // 0) | add // 0) as $guest_rss |
      ($measured | map(.guest_ram.pss_kb // 0) | add // 0) as $guest_pss |
      ($measured | map(.guest_ram.shared_clean_kb // 0) | add // 0) as $guest_shared_clean |
      ($measured | map(.guest_ram.shared_dirty_kb // 0) | add // 0) as $guest_shared_dirty |
      ($measured | map(.guest_ram.ksm_kb // 0) | add // 0) as $guest_ksm |
      ($guest_rss - $guest_pss | if . < 0 then 0 else . end) as $mapping_pss_savings |
      ($children | map(select(.snapshot_backing.available == true) | "\(.snapshot_backing.device_id):\(.snapshot_backing.inode)") | unique) as $backing_ids |
      ($measured | map(.guest_ram.inodes | unique)) as $ram_inode_sets |
      ([$ram_inode_sets[] | .[]]) as $ram_inodes |
      ([range(0; $ram_inode_sets | length) as $left |
        range($left + 1; $ram_inode_sets | length) as $right |
        $ram_inode_sets[$left][] as $inode |
        select($ram_inode_sets[$right] | index($inode)) |
        $inode] | unique) as $shared_ram_inodes |
      (($shared_ram_inodes | length) > 0) as $ram_inode_overlap |
      (if $guest_ksm > 0 then $guest_ksm elif $ram_inode_overlap then $mapping_pss_savings else 0 end) as $cross_child_shared |
      {
        available: (($children | length) > 0 and ($measured | length) == ($children | length)),
        evidence_scope: "guest RAM mappings only (/memfd:ch_ram); process-wide library sharing excluded",
        restore_mechanism: "userfaultfd UFFDIO_COPY into each child guest-memory mapping",
        sample_count: length,
        measured_sample_count: ($measured | length),
        snapshot_backing_shared: (($backing_ids | length) == 1 and ($children | length) > 1),
        snapshot_backing_ids: $backing_ids,
        guest_ram_mapping_inodes_distinct: (($ram_inodes | unique | length) == ($ram_inodes | length)),
        total_rss_kb: $guest_rss,
        total_pss_kb: $guest_pss,
        total_shared_clean_kb: $guest_shared_clean,
        total_shared_dirty_kb: $guest_shared_dirty,
        pss_mapping_savings_kb: $mapping_pss_savings,
        ksm_shared_kb: $guest_ksm,
        estimated_shared_savings_kb: $cross_child_shared,
        defensible_shared_guest_ram_kb: $cross_child_shared,
        positive_resident_guest_ram_sharing_proven: ($cross_child_shared > 0),
        cross_child_sharing_basis: (
          if $guest_ksm > 0 then "kernel-same-page-merging"
          elif $ram_inode_overlap then "shared-guest-ram-backing-inode"
          else "none"
          end
        ),
        ram_sharing_verdict: (
          if ($measured | length) != ($children | length) then "unavailable"
          elif $cross_child_shared > 0 then "observed"
          else "not-observed"
          end
        ),
        claim: (
          if ($measured | length) != ($children | length) then "insufficient-evidence"
          elif $cross_child_shared > 0 then "resident-guest-ram-sharing-observed"
          elif (($backing_ids | length) == 1 and ($children | length) > 1) then "snapshot-backing-shared-only"
          else "no-sharing-observed"
          end
        ),
        children: $children
      }
    ' "$tmp"
    rm -f "$tmp"
}

snapshot_dir_for() {
    local id="$1"
    printf '%s/%s' "$CH_SNAPSHOT_ROOT" "$id"
}

signer_fingerprint_for_key() {
    local key="$1" fingerprint
    fingerprint="$(gpg --batch --with-colons --fingerprint "$key" 2>/dev/null \
        | awk -F: '$1 == "fpr" { print toupper($10); exit }')"
    [[ "$fingerprint" =~ ^[0-9A-F]{40,64}$ ]] \
        || die "could not resolve an unambiguous signing fingerprint for $key"
    printf '%s' "$fingerprint"
}

clean_attestation_file_for() {
    printf '%s/%s/cloud-hypervisor/clean-base-attestation.json' "$VM_STORAGE_DIR" "$1"
}

vm_state_fingerprint() {
    local vm="$1" state="$VM_STORAGE_DIR/$vm/cloud-hypervisor/vm.env" disk
    [[ -f "$state" ]] || die "VM metadata missing: $state"
    disk="$(sed -n 's/^DISK_PATH=//p' "$state" | head -n1)"
    [[ -f "$disk" ]] || die "VM disk missing: $disk"
    printf '%s:%s:%s:%s' \
        "$(sha256sum "$state" | awk '{print $1}')" \
        "$(stat -c '%i' "$disk")" "$(stat -c '%s' "$disk")" "$(stat -c '%Y' "$disk")"
}

require_clean_attestation() {
    local vm="$1" attestation expected now prepared age
    command -v jq >/dev/null 2>&1 || die "jq is required for clean-base attestation"
    attestation="$(clean_attestation_file_for "$vm")"
    [[ -f "$attestation" ]] || die "pre-enrollment snapshot requires: ch-faststart.sh clean-prepare --vm $vm ..."
    [[ "$(stat -c '%a' "$attestation")" == "600" ]] || die "clean-base attestation must be mode 0600"
    jq -e --arg vm "$vm" '.schema == 1 and .vm == $vm and .checks.agent_inactive == true and .checks.no_tls_identity == true and .checks.no_bootstrap_or_oauth == true and .checks.credential_mounts_detached == true and .vm_paused == true' \
        "$attestation" >/dev/null || die "clean-base attestation is incomplete or invalid"
    expected="$(vm_state_fingerprint "$vm")"
    [[ "$(jq -r '.vm_state_fingerprint // empty' "$attestation")" == "$expected" ]] \
        || die "clean-base attestation no longer matches VM state or disk"
    prepared="$(jq -r '.prepared_at_unix // 0' "$attestation")"
    now="$(date +%s)"
    age=$((now - prepared))
    (( age >= 0 && age <= CH_CLEAN_ATTESTATION_MAX_AGE_SECS )) \
        || die "clean-base attestation expired; run clean-prepare again"
}

cmd_clean_prepare() {
    local vm="" host="" user="" identity=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --vm) vm="$2"; shift 2 ;;
            --host) host="$2"; shift 2 ;;
            --user) user="$2"; shift 2 ;;
            --identity) identity="$2"; shift 2 ;;
            *) die "unknown clean-prepare arg: $1" ;;
        esac
    done
    [[ -n "$vm" && -n "$host" && -n "$user" ]] \
        || die "clean-prepare requires --vm, --host, and --user"
    validate_identifier "VM name" "$vm"
    [[ "$host" != *[[:space:]]* && "$user" =~ ^[A-Za-z_][A-Za-z0-9._-]*$ ]] \
        || die "unsafe clean-prepare SSH target"
    local state="$VM_STORAGE_DIR/$vm/cloud-hypervisor/vm.env"
    [[ -f "$state" ]] || die "VM metadata missing: $state"
    # shellcheck source=/dev/null
    source "$state"
    [[ -S "$API_SOCKET" ]] || die "Cloud Hypervisor API socket missing: $API_SOCKET"
    local -a ssh_args=(-o BatchMode=yes -o StrictHostKeyChecking=yes)
    [[ -z "$identity" ]] || ssh_args+=(-i "$identity")
    local guest_result
    guest_result="$(ssh "${ssh_args[@]}" "$user@$host" sudo -n bash -s <<'GUEST'
set -euo pipefail
systemctl stop agent-client.service
# Keep the credential-free path/timer watchers active in the paused image so
# the first host-staged restore drop is observed even when a hotplug udev event
# is coalesced during snapshot restore.
systemctl start agent-client-restore-bootstrap.path agent-client-restore-bootstrap.timer
systemctl restart agent-client-restore-bootstrap-watcher.service
udevadm control --reload-rules
install -d -m 0755 /etc/systemd/network
cat >/etc/systemd/network/05-agentic-restore.network <<'NETWORK'
[Match]
Name=en*
Driver=virtio_net

[Network]
DHCP=ipv4
IPv6AcceptRA=yes

[DHCPv4]
ClientIdentifier=mac

[Link]
RequiredForOnline=no
NETWORK
# Snapshot the network manager after it has parsed the generic hotplug profile;
# otherwise a restored process retains only the original netplan MAC match in
# memory and a fresh child NIC never acquires a lease.
networkctl reload
systemctl is-active --quiet agent-client.service && exit 20
pgrep -x agent-client >/dev/null && exit 21
for mountpoint in /mnt/inbox /mnt/outbox /mnt/agent-share; do
    if mountpoint -q "$mountpoint"; then
        umount "$mountpoint"
    fi
done
mount | grep -Eq ' on /mnt/(inbox|outbox|agent-share) ' && exit 22
for path in /etc/agentic-sandbox/grpc-mtls /etc/agentic-sandbox/tls /run/agentic-sandbox/bootstrap-tls; do
    if [[ -d "$path" ]] && find "$path" -type f -print -quit | grep -q .; then
        exit 23
    fi
done
if [[ -f /etc/agentic-sandbox/agent.env ]] && \
   grep -Eq '^(AGENT_BOOTSTRAP_TOKEN|AGENT_SECRET|OAUTH_|.*TOKEN=|AGENT_GRPC_TLS_(CA|CERT|KEY)=)' /etc/agentic-sandbox/agent.env; then
    exit 24
fi
[[ ! -e /mnt/inbox/restore-bootstrap.env ]] || exit 25
printf '{"boot_id":"%s","ssh_host_key_b64":"%s","agent_inactive":true,"no_tls_identity":true,"no_bootstrap_or_oauth":true,"credential_mounts_detached":true}\n' \
    "$(cat /proc/sys/kernel/random/boot_id)" \
    "$(base64 -w0 /etc/ssh/ssh_host_ed25519_key.pub)"
GUEST
)" || die "guest clean-base preparation failed; the VM may contain credentials or active tenant state"
    printf '%s' "$guest_result" | jq -e '.agent_inactive and .no_tls_identity and .no_bootstrap_or_oauth and .credential_mounts_detached' >/dev/null \
        || die "guest clean-base preparation returned invalid evidence"
    local ssh_host_key known_hosts_file
    ssh_host_key="$(printf '%s' "$guest_result" | jq -r '.ssh_host_key_b64 // empty' | base64 -d)"
    [[ "$ssh_host_key" == ssh-ed25519\ * ]] || die "guest did not return a valid Ed25519 SSH host key"
    known_hosts_file="$VM_STORAGE_DIR/$vm/cloud-hypervisor/restore-ssh-known-hosts"
    printf 'ch-clean-base %s\n' "$ssh_host_key" > "$known_hosts_file"
    chmod 644 "$known_hosts_file"

    local info device_id
    info="$("$_ch_remote_bin" --api-socket "$API_SOCKET" info)"
    # Keep exactly one credential-free NIC in the snapshot. Cloud Hypervisor
    # can rewrite that device in the restore config, whereas a NIC hot-added
    # after restoring an already-booted kernel is not reliably enumerated.
    # Credential-bearing virtiofs devices remain strictly detached.
    while IFS= read -r device_id; do
        [[ -n "$device_id" ]] || continue
        "$_ch_remote_bin" --api-socket "$API_SOCKET" remove-device "$device_id" >/dev/null
    done < <(printf '%s' "$info" | jq -r '
        [((.config.fs // [])[] | .id)] | unique[]
    ')
    "$_ch_remote_bin" --api-socket "$API_SOCKET" pause >/dev/null

    local attestation tmp fingerprint
    attestation="$(clean_attestation_file_for "$vm")"
    fingerprint="$(vm_state_fingerprint "$vm")"
    tmp="$(mktemp "${attestation}.tmp.XXXXXX")"
    jq -n \
        --arg vm "$vm" \
        --arg boot_id "$(printf '%s' "$guest_result" | jq -r '.boot_id')" \
        --arg fingerprint "$fingerprint" \
        --argjson prepared "$(date +%s)" \
        '{schema:1, vm:$vm, prepared_at_unix:$prepared, guest_boot_id:$boot_id, vm_state_fingerprint:$fingerprint, vm_paused:true, checks:{agent_inactive:true,no_tls_identity:true,no_bootstrap_or_oauth:true,credential_mounts_detached:true}}' > "$tmp"
    chmod 600 "$tmp"
    if [[ -n "${SUDO_UID:-}" && -n "${SUDO_GID:-}" ]]; then
        chown "$SUDO_UID:$SUDO_GID" "$tmp"
    fi
    mv -fT "$tmp" "$attestation"
    printf '{"vm":"%s","prepared":true,"attestation":"%s"}\n' \
        "$(json_escape "$vm")" "$(json_escape "$attestation")"
}

write_provenance() {
    local snapshot_dir="$1"
    local id="$2"
    local posture="$3"
    local pre_enrollment="$4"
    local signer_fingerprint="${5:-}"
    local manifest="$snapshot_dir/provenance.sha256"
    local contains_tenant_secrets=true
    if [[ "$pre_enrollment" == true ]]; then
        contains_tenant_secrets=false
    fi
    cat > "$snapshot_dir/secret-posture.json" <<EOF
{
  "snapshot_id": "$(json_escape "$id")",
  "pre_enrollment": $pre_enrollment,
  "posture": "$(json_escape "$posture")",
  "base_contains_tenant_secrets": $contains_tenant_secrets,
  "snapshot_signer_fingerprint": "$(json_escape "$signer_fingerprint")",
  "enroll_on_restore_required": true,
  "fresh_identity_on_restore": ["vsock_cid", "bootstrap_or_vsock_enrollment", "mtls_identity"]
}
EOF
    (
        cd "$snapshot_dir"
        find . -maxdepth 2 -type f \
            ! -name provenance.sha256 \
            ! -name provenance.sha256.sig \
            ! -name sealed-snapshot.gpg \
            ! -name 'sealed-snapshot.gpg.*' \
            -print0 | sort -z | xargs -0 sha256sum
    ) > "$manifest"
    if [[ -n "${CH_SNAPSHOT_SIGN_KEY:-}" ]]; then
        gpg --batch --yes --local-user "$CH_SNAPSHOT_SIGN_KEY" --detach-sign \
            -o "$manifest.sig" "$manifest"
    fi
}

verify_snapshot() {
    local snapshot_dir="$1"
    [[ -d "$snapshot_dir" ]] || die "snapshot not found: $snapshot_dir"
    [[ -f "$snapshot_dir/provenance.sha256" ]] || die "snapshot provenance missing: $snapshot_dir/provenance.sha256"
    (
        cd "$snapshot_dir"
        sha256sum -c provenance.sha256 >/dev/null
    ) || die "snapshot provenance verification failed: $snapshot_dir"
    [[ -f "$snapshot_dir/provenance.sha256.sig" ]] \
        || die "snapshot signature missing: $snapshot_dir/provenance.sha256.sig"
    [[ "$CH_SNAPSHOT_SIGNER_FINGERPRINT" =~ ^[0-9A-Fa-f]{40,64}$ ]] \
        || die "CH_SNAPSHOT_SIGNER_FINGERPRINT must pin the approved snapshot signing key"
    local expected actual status
    expected="$(printf '%s' "$CH_SNAPSHOT_SIGNER_FINGERPRINT" | tr '[:lower:]' '[:upper:]')"
    status="$(gpg --batch --status-fd=1 --verify "$snapshot_dir/provenance.sha256.sig" "$snapshot_dir/provenance.sha256" 2>/dev/null)" \
        || die "snapshot signature verification failed: $snapshot_dir"
    actual="$(printf '%s\n' "$status" | awk '$1 == "[GNUPG:]" && $2 == "VALIDSIG" { print toupper($3); exit }')"
    [[ "$actual" == "$expected" ]] || die "snapshot signature was made by an unapproved key"
    [[ "$(jq -r '.snapshot_signer_fingerprint // empty' "$snapshot_dir/secret-posture.json")" == "$expected" ]] \
        || die "snapshot signer metadata does not match the approved key"
    if ! grep -q '"pre_enrollment"[[:space:]]*:[[:space:]]*true' "$snapshot_dir/secret-posture.json" 2>/dev/null; then
        die "refusing to restore non-clean-base snapshot without an explicit unseal workflow: $snapshot_dir"
    fi
    if grep -q '^GPU_ENABLED=true$' "$snapshot_dir/source-vm.env" 2>/dev/null; then
        die "GPU/VFIO snapshots are not eligible for restore, fork, or warm-pool hand-out; use a reset-gated cold VM"
    fi
}

cmd_snapshot() {
    local vm="" id="" pre_enrollment=false secret_bearing=false seal_key=""
    local sign_key="$CH_SNAPSHOT_SIGN_KEY"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --vm) vm="$2"; shift 2 ;;
            --id) id="$2"; shift 2 ;;
            --pre-enrollment) pre_enrollment=true; shift ;;
            --secret-bearing) secret_bearing=true; shift ;;
            --seal-key) seal_key="$2"; shift 2 ;;
            --sign-key) sign_key="$2"; shift 2 ;;
            *) die "unknown snapshot arg: $1" ;;
        esac
    done
    [[ -n "$vm" && -n "$id" ]] || die "snapshot requires --vm and --id"
    validate_identifier "snapshot id" "$id"
    if [[ "$pre_enrollment" == "$secret_bearing" ]]; then
        die "snapshot requires exactly one of --pre-enrollment or --secret-bearing"
    fi
    if [[ "$secret_bearing" == true && -z "$seal_key" ]]; then
        die "secret-bearing CH snapshots must be sealed with --seal-key"
    fi
    if [[ "$pre_enrollment" == true && -z "$sign_key" ]]; then
        die "pre-enrollment CH base snapshots must be signed with --sign-key or CH_SNAPSHOT_SIGN_KEY"
    fi
    if [[ "$pre_enrollment" == true ]]; then
        require_clean_attestation "$vm"
    fi

    local snapshot_dir
    snapshot_dir="$(snapshot_dir_for "$id")"
    [[ ! -e "$snapshot_dir" ]] || die "snapshot already exists: $snapshot_dir"
    mkdir -p "$snapshot_dir"
    SNAPSHOT_CLEANUP_DIR="$snapshot_dir"
    trap cleanup_partial_snapshot EXIT
    if [[ "$secret_bearing" == true ]]; then
        chmod 700 "$snapshot_dir"
    fi
    backend_snapshot_vm "$vm" "$snapshot_dir" "file://$snapshot_dir/ch-state"
    # A VMM launched by a privileged service writes snapshot state as root.
    # Normalize only the newly-created snapshot payload to the invoking user so
    # validation, signing, and later unprivileged restores can read it.
    if [[ ! -r "$snapshot_dir/ch-state/config.json" ]]; then
        sudo -n chown -R -- "$(id -u):$(id -g)" "$snapshot_dir/ch-state" \
            || die "snapshot state is unreadable and ownership could not be normalized"
    fi
    if [[ -f "$VM_STORAGE_DIR/$vm/cloud-hypervisor/restore-ssh-known-hosts" ]]; then
        cp "$VM_STORAGE_DIR/$vm/cloud-hypervisor/restore-ssh-known-hosts" "$snapshot_dir/restore-ssh-known-hosts"
        chmod 444 "$snapshot_dir/restore-ssh-known-hosts"
    fi
    if [[ "$pre_enrollment" == true ]]; then
        command -v jq >/dev/null 2>&1 || die "jq is required for clean-base snapshot validation"
        if jq -e '((.fs // []) | length > 0) or ((.net // []) | length != 1)' "$snapshot_dir/ch-state/config.json" >/dev/null; then
            die "clean-base capture must contain exactly one credential-free network device and no credential-share devices"
        fi
    fi
    find "$snapshot_dir/ch-state" -type f -exec chmod 0444 {} +

    local posture="clean-base"
    if [[ "$secret_bearing" == true ]]; then
        posture="secret-bearing-sealed"
    fi
    local signer_fingerprint=""
    [[ -z "$sign_key" ]] || signer_fingerprint="$(signer_fingerprint_for_key "$sign_key")"
    CH_SNAPSHOT_SIGN_KEY="$sign_key" write_provenance "$snapshot_dir" "$id" "$posture" "$pre_enrollment" "$signer_fingerprint"

    if [[ "$secret_bearing" == true ]]; then
        local -a seal_args=(
            seal --key "$seal_key" --out "$snapshot_dir/sealed-snapshot.gpg"
        )
        if [[ -n "$sign_key" ]]; then
            seal_args+=(--sign-key "$sign_key")
        fi
        seal_args+=(
            "$snapshot_dir/ch-state"
            "$snapshot_dir/backend-metadata.json"
            "$snapshot_dir/source-vm.env"
            "$snapshot_dir/provenance.sha256"
            "$snapshot_dir/secret-posture.json"
        )
        if [[ -f "$snapshot_dir/provenance.sha256.sig" ]]; then
            seal_args+=("$snapshot_dir/provenance.sha256.sig")
        fi
        "$SCRIPT_DIR/snapshot-seal.sh" "${seal_args[@]}"
        rm -rf "$snapshot_dir/ch-state"
        rm -f "$snapshot_dir/backend-metadata.json" "$snapshot_dir/source-vm.env" \
            "$snapshot_dir/provenance.sha256" "$snapshot_dir/provenance.sha256.sig" \
            "$snapshot_dir/secret-posture.json"
        chmod 700 "$snapshot_dir"
        chmod 600 "$snapshot_dir/sealed-snapshot.gpg" "$snapshot_dir/sealed-snapshot.gpg.sha256"
        if [[ -f "$snapshot_dir/sealed-snapshot.gpg.sig" ]]; then
            chmod 600 "$snapshot_dir/sealed-snapshot.gpg.sig"
        fi
    fi
    SNAPSHOT_CLEANUP_DIR=""
    trap - EXIT
    if [[ "$pre_enrollment" == true ]]; then
        rm -f -- "$(clean_attestation_file_for "$vm")"
    fi

    log_success "snapshot captured: $snapshot_dir"
    printf '{"snapshot_id":"%s","snapshot_dir":"%s","pre_enrollment":%s,"posture":"%s"}\n' \
        "$(json_escape "$id")" "$(json_escape "$snapshot_dir")" "$pre_enrollment" "$(json_escape "$posture")"
}

cmd_restore() {
    local snapshot="" name="" mode="ondemand" instance_id="" wait_enrollment_ready=false
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --snapshot) snapshot="$2"; shift 2 ;;
            --name) name="$2"; shift 2 ;;
            --mode) mode="$2"; shift 2 ;;
            --instance-id) instance_id="$2"; shift 2 ;;
            --wait-enrollment-ready) wait_enrollment_ready=true; shift ;;
            *) die "unknown restore arg: $1" ;;
        esac
    done
    [[ -n "$snapshot" && -n "$name" ]] || die "restore requires --snapshot and --name"
    validate_identifier "snapshot id" "$snapshot"
    validate_identifier "child name" "$name"
    [[ "$mode" == "ondemand" || "$mode" == "copy" ]] || die "--mode must be ondemand or copy"
    [[ -n "$instance_id" ]] || instance_id="$name"
    local snapshot_dir
    snapshot_dir="$(snapshot_dir_for "$snapshot")"
    verify_snapshot "$snapshot_dir"
    require_snapshot_agentshare_for_bootstrap "$snapshot_dir"

    local cid child_disk start_ms end_ms metrics_path duration_ms metrics_tmp
    ENROLL_BOOTSTRAP_ISSUED=false
    CH_CLEANUP_CHILDREN=("$name")
    trap cleanup_partial_children EXIT
    cid="$(allocate_cid_for_vm "$name" "$instance_id")"
    child_disk="$VM_STORAGE_DIR/$name/$name.qcow2"
    start_ms="$(ms_now)"
    CH_ACTIVE_RESTORE_SNAPSHOT="$snapshot"
    CH_ACTIVE_RESTORE_INSTANCE_ID="$instance_id"
    CH_ACTIVE_RESTORE_CHILD_PREFIX=""
    metrics_tmp="$(mktemp)"
    backend_restore_vm "$name" "$snapshot_dir" "$child_disk" "$cid" "$mode" true > "$metrics_tmp"
    metrics_path="$(cat "$metrics_tmp")"
    rm -f "$metrics_tmp"
    end_ms="$(ms_now)"
    duration_ms=$((end_ms - start_ms))

    # Pool replenishment is off the request path; its value is the bounded
    # claim/resume handoff below, not the background snapshot restore time.
    if [[ "${CH_WARM_PREPARE:-false}" != "true" ]] \
        && (( duration_ms > CH_RESTORE_LATENCY_BUDGET_MS )); then
        die "restore latency ${duration_ms}ms exceeded budget ${CH_RESTORE_LATENCY_BUDGET_MS}ms"
    fi
    if [[ "${ENROLL_BOOTSTRAP_ISSUED:-false}" == "true" ]]; then
        trigger_guest_restore_bootstrap "$name" "$snapshot"
    fi
    local enrollment_ready=false ready_duration_ms=0
    if [[ "$wait_enrollment_ready" == true ]]; then
        wait_for_enrollment_ready "$name" "$start_ms"
        enrollment_ready=true
        ready_duration_ms="$ENROLLMENT_READY_DURATION_MS"
        if (( ready_duration_ms > CH_RESTORE_READY_LATENCY_BUDGET_MS )); then
            die "restore-to-enrollment-ready latency ${ready_duration_ms}ms exceeded budget ${CH_RESTORE_READY_LATENCY_BUDGET_MS}ms"
        fi
    fi
    local bootstrap_issued="${ENROLL_BOOTSTRAP_ISSUED:-false}"
    local bootstrap_spiffe="${ENROLL_BOOTSTRAP_SPIFFE:-}"
    printf '{"name":"%s","snapshot_id":"%s","vsock_cid":%s,"disk":"%s","metrics":"%s","duration_ms":%s,"enroll_on_restore":true,"bootstrap_token_issued":%s,"bootstrap_spiffe_id":"%s","enrollment_ready":%s,"ready_duration_ms":%s}\n' \
        "$(json_escape "$name")" "$(json_escape "$snapshot")" "$cid" "$(json_escape "$child_disk")" "$(json_escape "$metrics_path")" "$duration_ms" "$bootstrap_issued" "$(json_escape "$bootstrap_spiffe")" "$enrollment_ready" "$ready_duration_ms"
    CH_CLEANUP_CHILDREN=()
    trap - EXIT
}

cmd_fork() {
    local snapshot="" prefix="" count="" mode="ondemand" wait_enrollment_ready=false resume_children=true
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --snapshot) snapshot="$2"; shift 2 ;;
            --prefix) prefix="$2"; shift 2 ;;
            --count) count="$2"; shift 2 ;;
            --mode) mode="$2"; shift 2 ;;
            --paused) resume_children=false; shift ;;
            --wait-enrollment-ready) wait_enrollment_ready=true; shift ;;
            *) die "unknown fork arg: $1" ;;
        esac
    done
    [[ -n "$snapshot" && -n "$prefix" && "$count" =~ ^[0-9]+$ && "$count" -gt 0 ]] || die "fork requires --snapshot, --prefix, --count N"
    validate_identifier "snapshot id" "$snapshot"
    validate_identifier "child prefix" "$prefix"
    [[ "$mode" == "ondemand" || "$mode" == "copy" ]] || die "--mode must be ondemand or copy"
    if [[ "$resume_children" == false && "$wait_enrollment_ready" == true ]]; then
        die "--paused cannot be combined with --wait-enrollment-ready"
    fi
    local snapshot_dir
    snapshot_dir="$(snapshot_dir_for "$snapshot")"
    verify_snapshot "$snapshot_dir"
    require_snapshot_agentshare_for_bootstrap "$snapshot_dir"
    local children start_ms
    start_ms="$(ms_now)"
    local cleanup_idx
    CH_CLEANUP_CHILDREN=()
    for cleanup_idx in $(seq 1 "$count"); do
        CH_CLEANUP_CHILDREN+=("${prefix}-${cleanup_idx}")
    done
    trap cleanup_partial_children EXIT
    CH_ACTIVE_RESTORE_SNAPSHOT="$snapshot"
    CH_ACTIVE_RESTORE_INSTANCE_ID=""
    CH_ACTIVE_RESTORE_CHILD_PREFIX="$prefix"
    children="$(backend_fork_vm "$snapshot_dir" "$prefix" "$count" "$mode" "$resume_children")"
    if [[ "$wait_enrollment_ready" == true ]]; then
        while IFS= read -r child_name; do
            trigger_guest_restore_bootstrap "$child_name" "$snapshot"
        done < <(printf '%s' "$children" | jq -r '.[].name')
    fi
    local all_enrollment_ready=false max_ready_duration_ms=0 child_name
    if [[ "$wait_enrollment_ready" == true ]]; then
        while IFS= read -r child_name; do
            wait_for_enrollment_ready "$child_name" "$start_ms"
            (( ENROLLMENT_READY_DURATION_MS > max_ready_duration_ms )) \
                && max_ready_duration_ms="$ENROLLMENT_READY_DURATION_MS"
        done < <(printf '%s' "$children" | jq -r '.[].name')
        all_enrollment_ready=true
    fi
    local manifest="$VM_STORAGE_DIR/${prefix}-fork-manifest.json"
    local memory_sharing children_paused=false
    if [[ "$resume_children" == false ]]; then
        children_paused=true
    fi
    memory_sharing="$(fork_memory_sharing_json "$children")"
    cat > "$manifest" <<EOF
{
  "snapshot_id": "$(json_escape "$snapshot")",
  "child_prefix": "$(json_escape "$prefix")",
  "count": $count,
  "memory_restore_mode": "$(json_escape "$mode")",
  "children_paused": $children_paused,
  "children": $children,
  "memory_sharing": $memory_sharing,
  "fresh_identity_per_child": true,
  "disk_cow_per_child": true
  ,"all_guest_enrollment_acknowledged": $all_enrollment_ready
  ,"max_guest_enrollment_ready_ms": $max_ready_duration_ms
}
EOF
    printf '{"manifest":"%s","children":%s,"all_guest_enrollment_acknowledged":%s,"max_guest_enrollment_ready_ms":%s}\n' \
        "$(json_escape "$manifest")" "$children" "$all_enrollment_ready" "$max_ready_duration_ms"
    CH_CLEANUP_CHILDREN=()
    trap - EXIT
}

cmd_fork_evidence() {
    local prefix="" count="" manifest=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --prefix) prefix="$2"; shift 2 ;;
            --count) count="$2"; shift 2 ;;
            --manifest) manifest="$2"; shift 2 ;;
            *) die "unknown fork-evidence arg: $1" ;;
        esac
    done
    [[ -n "$prefix" && "$count" =~ ^[0-9]+$ && "$count" -gt 1 ]] \
        || die "fork-evidence requires --prefix and --count N (N > 1)"
    validate_identifier "child prefix" "$prefix"
    [[ -n "$manifest" ]] || manifest="$VM_STORAGE_DIR/${prefix}-fork-manifest.json"
    [[ -f "$manifest" ]] || die "fork manifest not found: $manifest"

    local children_tmp
    children_tmp="$(mktemp)"
    printf '[' > "$children_tmp"
    local first=true index name state metrics disk cid instance
    for index in $(seq 1 "$count"); do
        name="${prefix}-${index}"
        state="$VM_STORAGE_DIR/$name/cloud-hypervisor/vm.env"
        metrics="$(backend_sample_vm_memory "$name")"
        [[ -f "$state" && -f "$metrics" ]] || die "fork child evidence missing for $name"
        disk="$(sed -n 's/^DISK_PATH=//p' "$state" | head -n1)"
        cid="$(sed -n 's/^VSOCK_CID=//p' "$state" | head -n1)"
        instance="$(jq -r '.instance_id // .vm_name // empty' "$VM_STORAGE_DIR/$name/enroll-on-restore.json")"
        [[ -n "$instance" ]] || instance="$name"
        if [[ "$first" == true ]]; then first=false; else printf ',' >> "$children_tmp"; fi
        jq -cn --arg name "$name" --arg instance "$instance" --arg disk "$disk" --arg metrics "$metrics" --argjson cid "$cid" \
            '{name:$name,instance_id:$instance,vsock_cid:$cid,disk:$disk,metrics:$metrics}' >> "$children_tmp"
    done
    printf ']\n' >> "$children_tmp"
    local children memory_sharing manifest_tmp sampled_at
    children="$(cat "$children_tmp")"
    rm -f "$children_tmp"
    memory_sharing="$(fork_memory_sharing_json "$children")"
    sampled_at="$(date --utc +%Y-%m-%dT%H:%M:%SZ)"
    manifest_tmp="$(mktemp "${manifest}.tmp.XXXXXX")"
    jq --argjson memory_sharing "$memory_sharing" --arg sampled_at "$sampled_at" \
        '.memory_sharing=$memory_sharing | .memory_evidence_sampled_at=$sampled_at' \
        "$manifest" > "$manifest_tmp"
    mv -f "$manifest_tmp" "$manifest"
    jq -c --arg manifest "$manifest" '{manifest:$manifest,memory_evidence_sampled_at,memory_sharing}' "$manifest"
}

cmd_warm_init() {
    local snapshot="" size="" prefix=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --snapshot) snapshot="$2"; shift 2 ;;
            --size) size="$2"; shift 2 ;;
            --prefix) prefix="$2"; shift 2 ;;
            *) die "unknown warm-init arg: $1" ;;
        esac
    done
    [[ -n "$snapshot" && -n "$prefix" && "$size" =~ ^[0-9]+$ && "$size" -gt 0 ]] || die "warm-init requires --snapshot, --size N, --prefix"
    validate_identifier "snapshot id" "$snapshot"
    validate_identifier "pool prefix" "$prefix"
    local snapshot_dir pool_dir pool_file
    snapshot_dir="$(snapshot_dir_for "$snapshot")"
    verify_snapshot "$snapshot_dir"
    pool_dir="$CH_WARM_POOL_ROOT/$prefix"
    [[ ! -e "$pool_dir/pool.json" ]] || die "warm pool already exists: $prefix"
    mkdir -p "$pool_dir"
    pool_file="$pool_dir/pool.json"
    cat > "$pool_file" <<EOF
{
  "pool": "$(json_escape "$prefix")",
  "snapshot_id": "$(json_escape "$snapshot")",
  "target_size": $size,
  "pre_enrollment": true,
  "idle": 0,
  "handed_out": 0,
  "next_slot": 1,
  "slots": []
}
EOF
    cmd_warm_reconcile --pool "$prefix" >/dev/null
    printf '{"pool":"%s","snapshot_id":"%s","target_size":%s,"idle":%s,"slots":%s}\n' \
        "$(json_escape "$prefix")" "$(json_escape "$snapshot")" "$size" \
        "$(jq -r '.idle' "$pool_file")" "$(jq -c '.slots' "$pool_file")"
}

cmd_warm_reconcile() {
    local pool=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --pool) pool="$2"; shift 2 ;;
            *) die "unknown warm-reconcile arg: $1" ;;
        esac
    done
    [[ -n "$pool" ]] || die "warm-reconcile requires --pool"
    validate_identifier "pool name" "$pool"
    command -v jq >/dev/null 2>&1 || die "jq is required for warm-pool reconciliation"
    local pool_file="$CH_WARM_POOL_ROOT/$pool/pool.json" lock_file
    [[ -f "$pool_file" ]] || die "warm pool not found: $pool"
    lock_file="${pool_file}.lock"
    exec {pool_lock_fd}>"$lock_file"
    flock "$pool_lock_fd"
    local snapshot target ready next slot tmp
    snapshot="$(jq -r '.snapshot_id' "$pool_file")"
    target="$(jq -r '.target_size' "$pool_file")"
    ready="$(jq '[.slots[] | select(.state == "ready_paused")] | length' "$pool_file")"
    while (( ready < target )); do
        next="$(jq -r '.next_slot' "$pool_file")"
        slot="${pool}-slot-${next}"
        tmp="$(mktemp "${pool_file}.tmp.XXXXXX")"
        jq --arg slot "$slot" --argjson next "$((next + 1))" \
            '.next_slot = $next | .slots += [{slot:$slot,state:"preparing"}]' \
            "$pool_file" > "$tmp"
        mv -f "$tmp" "$pool_file"
        flock -u "$pool_lock_fd"

        local restore_output
        if restore_output="$(CH_WARM_PREPARE=true cmd_restore --snapshot "$snapshot" --name "$slot" --mode ondemand)" \
            && wait_for_warm_slot_ready "$slot" \
            && pause_warm_slot "$slot"; then
            flock "$pool_lock_fd"
            tmp="$(mktemp "${pool_file}.tmp.XXXXXX")"
            jq --arg slot "$slot" \
                '(.slots[] | select(.slot == $slot)).state = "ready_paused" | .idle = ([.slots[] | select(.state == "ready_paused")] | length)' \
                "$pool_file" > "$tmp"
            mv -f "$tmp" "$pool_file"
            ready=$((ready + 1))
        else
            backend_destroy_vm "$slot" >/dev/null 2>&1 || true
            flock "$pool_lock_fd"
            tmp="$(mktemp "${pool_file}.tmp.XXXXXX")"
            jq --arg slot "$slot" \
                '(.slots[] | select(.slot == $slot)).state = "failed" | .idle = ([.slots[] | select(.state == "ready_paused")] | length)' \
                "$pool_file" > "$tmp"
            mv -f "$tmp" "$pool_file"
            flock -u "$pool_lock_fd"
            die "failed to prepare warm slot $slot"
        fi
    done
    flock -u "$pool_lock_fd"
    jq -c '{pool,snapshot_id,target_size,idle,handed_out,slots}' "$pool_file"
}

cmd_warm_status() {
    local pool=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --pool) pool="$2"; shift 2 ;;
            *) die "unknown warm-status arg: $1" ;;
        esac
    done
    [[ -n "$pool" ]] || die "warm-status requires --pool"
    validate_identifier "pool name" "$pool"
    local pool_file="$CH_WARM_POOL_ROOT/$pool/pool.json"
    [[ -f "$pool_file" ]] || die "warm pool not found: $pool"
    jq -c '{pool,snapshot_id,target_size,idle,handed_out,next_slot,slots}' "$pool_file"
}

cmd_warm_handoff() {
    local pool="" name="" mode="ondemand" instance_id="" wait_enrollment_ready=false
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --pool) pool="$2"; shift 2 ;;
            --name) name="$2"; shift 2 ;;
            --mode) mode="$2"; shift 2 ;;
            --instance-id) instance_id="$2"; shift 2 ;;
            --wait-enrollment-ready) wait_enrollment_ready=true; shift ;;
            *) die "unknown warm-handoff arg: $1" ;;
        esac
    done
    [[ -n "$pool" && -n "$name" ]] || die "warm-handoff requires --pool and --name"
    validate_identifier "pool name" "$pool"
    validate_identifier "child name" "$name"
    local pool_file="$CH_WARM_POOL_ROOT/$pool/pool.json"
    [[ -f "$pool_file" ]] || die "warm pool not found: $pool"
    command -v jq >/dev/null 2>&1 || die "jq is required for warm-pool accounting"
    local lock_file="${pool_file}.lock"
    exec {pool_lock_fd}>"$lock_file"
    flock "$pool_lock_fd"
    local snapshot idle slot
    snapshot="$(jq -r '.snapshot_id // empty' "$pool_file")"
    idle="$(jq -r '.idle // 0' "$pool_file")"
    [[ -n "$snapshot" ]] || die "warm pool missing snapshot_id: $pool_file"
    (( idle > 0 )) || die "warm pool $pool has no idle capacity"
    slot="$(jq -r '.slots[] | select(.state == "ready_paused") | .slot' "$pool_file" | head -n1)"
    [[ -n "$slot" ]] || die "warm pool $pool accounting has no claimable slot"
    local pool_tmp
    pool_tmp="$(mktemp "${pool_file}.tmp.XXXXXX")"
    jq --arg slot "$slot" \
        '(.slots[] | select(.slot == $slot)).state = "claiming" | .idle = ([.slots[] | select(.state == "ready_paused")] | length)' \
        "$pool_file" > "$pool_tmp"
    mv -f "$pool_tmp" "$pool_file"
    flock -u "$pool_lock_fd"

    [[ -n "$instance_id" ]] || instance_id="$name"
    local start_ms cid
    start_ms="$(ms_now)"
    rebind_warm_slot "$slot" "$name" "$instance_id"
    cid="$(child_state_field "$name" VSOCK_CID)"
    CH_ACTIVE_RESTORE_SNAPSHOT="$snapshot"
    CH_ACTIVE_RESTORE_INSTANCE_ID="$instance_id"
    CH_ACTIVE_RESTORE_CHILD_PREFIX=""
    write_enroll_on_restore_metadata "$name" "$instance_id" "$snapshot" "$cid"
    resume_warm_slot "$name"

    local enrollment_ready=false ready_duration_ms=0 handoff_duration_ms
    handoff_duration_ms=$(( $(ms_now) - start_ms ))
    if (( handoff_duration_ms > CH_WARM_HANDOFF_READY_LATENCY_BUDGET_MS )); then
        die "warm slot handoff ${handoff_duration_ms}ms exceeded budget ${CH_WARM_HANDOFF_READY_LATENCY_BUDGET_MS}ms"
    fi
    # Guest activation is outside the paused-slot claim/resume latency budget.
    # The transient in-guest watcher normally observes the staged drop first;
    # pinned SSH is a deterministic CLI fallback.
    trigger_guest_restore_bootstrap "$name" "$snapshot"
    if [[ "$wait_enrollment_ready" == true ]]; then
        wait_for_enrollment_ready "$name" "$start_ms"
        enrollment_ready=true
        ready_duration_ms="$ENROLLMENT_READY_DURATION_MS"
    fi

    exec {pool_lock_fd}>"$lock_file"
    flock "$pool_lock_fd"
    pool_tmp="$(mktemp "${pool_file}.tmp.XXXXXX")"
    jq --arg slot "$slot" --arg name "$name" --arg instance "$instance_id" \
        '(.slots[] | select(.slot == $slot)) |= (.state = "handed_out" | .name = $name | .instance_id = $instance) | .handed_out += 1 | .idle = ([.slots[] | select(.state == "ready_paused")] | length)' \
        "$pool_file" > "$pool_tmp"
    mv -f "$pool_tmp" "$pool_file"
    flock -u "$pool_lock_fd"

    CH_BOOTSTRAP_STDIN=0 CH_BOOTSTRAP_STDIN_PAYLOAD= \
        "$0" warm-reconcile --pool "$pool" >"$CH_WARM_POOL_ROOT/$pool/reconcile.log" 2>&1 &
    printf '{"name":"%s","claimed_slot":"%s","snapshot_id":"%s","vsock_cid":%s,"duration_ms":%s,"enroll_on_restore":true,"bootstrap_token_issued":%s,"bootstrap_spiffe_id":"%s","enrollment_ready":%s,"ready_duration_ms":%s}\n' \
        "$(json_escape "$name")" "$(json_escape "$slot")" "$(json_escape "$snapshot")" "$cid" \
        "$handoff_duration_ms" "${ENROLL_BOOTSTRAP_ISSUED:-false}" "$(json_escape "${ENROLL_BOOTSTRAP_SPIFFE:-}")" \
        "$enrollment_ready" "$ready_duration_ms"
}

cmd_verify() {
    local snapshot=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --snapshot) snapshot="$2"; shift 2 ;;
            *) die "unknown verify arg: $1" ;;
        esac
    done
    [[ -n "$snapshot" ]] || die "verify requires --snapshot"
    validate_identifier "snapshot id" "$snapshot"
    verify_snapshot "$(snapshot_dir_for "$snapshot")"
    printf '{"snapshot_id":"%s","verified":true}\n' "$(json_escape "$snapshot")"
}

load_bootstrap_stdin

case "${1:-}" in
    clean-prepare) shift; cmd_clean_prepare "$@" ;;
    snapshot) shift; cmd_snapshot "$@" ;;
    restore) shift; cmd_restore "$@" ;;
    fork) shift; cmd_fork "$@" ;;
    fork-evidence) shift; cmd_fork_evidence "$@" ;;
    warm-init) shift; cmd_warm_init "$@" ;;
    warm-handoff) shift; cmd_warm_handoff "$@" ;;
    warm-reconcile) shift; cmd_warm_reconcile "$@" ;;
    warm-status) shift; cmd_warm_status "$@" ;;
    verify) shift; cmd_verify "$@" ;;
    -h|--help|"") usage ;;
    *) usage; die "unknown subcommand: $1" ;;
esac
