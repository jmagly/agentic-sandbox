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
CH_SNAPSHOT_SIGN_KEY="${CH_SNAPSHOT_SIGN_KEY:-}"
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
  $0 snapshot --vm NAME --id ID --secret-bearing --seal-key KEY [--sign-key KEY]
  $0 restore  --snapshot ID --name NAME [--mode ondemand|copy] [--instance-id ID] [--wait-enrollment-ready]
  $0 fork     --snapshot ID --prefix NAME --count N [--mode ondemand|copy]
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
    date +%s%3N
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
    local path="$1" instance_id="$2" token="$3" spiffe="$4" expires="$5" tls_dir="$6" enrollment_url="$7"
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
        chmod 600 "$tmp"
    ); then
        rm -f "$tmp"
        return 1
    fi
    if ! mv -fT -- "$tmp" "$path"; then
        rm -f "$tmp"
        return 1
    fi
}

write_enroll_on_restore_metadata() {
    local name="$1" instance_id="$2" snapshot="$3" cid="$4"
    local vm_dir="$VM_STORAGE_DIR/$name"
    local bootstrap_json token spiffe expires tls_dir enrollment_url guest_bootstrap_env_path token_issued=false
    mkdir -p "$vm_dir"

    bootstrap_json="$(bootstrap_json_for_child "$name")"
    token="${AGENT_BOOTSTRAP_TOKEN:-}"
    spiffe="${AGENT_BOOTSTRAP_SPIFFE_ID:-}"
    expires="${AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS:-}"
    tls_dir="${AGENT_BOOTSTRAP_TLS_DIR:-}"
    enrollment_url="${AGENT_BOOTSTRAP_ENROLLMENT_URL:-}"
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
        fi
    fi
    if [[ -n "$token" || -n "$spiffe" ]]; then
        [[ -n "$token" && -n "$spiffe" ]] || die "bootstrap enrollment requires both token and spiffe_id for $name"
        local child_inbox_path
        child_inbox_path="$(child_state_field "$name" "INBOX_PATH")"
        [[ -n "$child_inbox_path" ]] \
            || die "bootstrap enrollment requires an isolated child inbox for $name"
        guest_bootstrap_env_path="$child_inbox_path/restore-bootstrap.env"
        write_restore_bootstrap_env "$guest_bootstrap_env_path" "$instance_id" "$token" "$spiffe" "$expires" "$tls_dir" "$enrollment_url"
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
  "guest_bootstrap_env_mode": "$(if [[ -n "${guest_bootstrap_env_path:-}" ]]; then stat -c '%a' "$guest_bootstrap_env_path"; fi)"
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
    if [[ -n "${CH_ACTIVE_RESTORE_CHILD_PREFIX:-}" ]]; then
        instance_id="$name"
    fi
    write_enroll_on_restore_metadata "$name" "$instance_id" "$snapshot" "$(child_state_field "$name" VSOCK_CID)"
}

wait_for_enrollment_ready() {
    local name="$1" start_ms="$2"
    local metadata="$VM_STORAGE_DIR/$name/enroll-on-restore.json"
    local drop
    drop="$(sed -n 's/.*"guest_bootstrap_env_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$metadata" | head -n1)"
    [[ -n "$drop" && -f "$drop" ]] || die "enrollment readiness requires a staged guest bootstrap drop for $name"
    local deadline=$(( $(ms_now) + CH_ENROLLMENT_READY_TIMEOUT_MS ))
    while (( $(ms_now) <= deadline )); do
        if ! grep -q '^AGENT_BOOTSTRAP_TOKEN=' "$drop" 2>/dev/null \
            && ! grep -q '^AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=' "$drop" 2>/dev/null; then
            ENROLLMENT_READY_DURATION_MS=$(( $(ms_now) - start_ms ))
            return 0
        fi
        sleep 0.05
    done
    die "restored guest $name did not consume and scrub its bootstrap credential within ${CH_ENROLLMENT_READY_TIMEOUT_MS}ms"
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
        jq -c '{name: .child_vm, vmm_pid, rss_kb: .vmm_rss_kb, pss_kb: .vmm_pss_kb, shared_clean_kb: .vmm_shared_clean_kb, shared_dirty_kb: .vmm_shared_dirty_kb, duration_ms}' "$metrics_path" >> "$tmp"
    done < <(printf '%s' "$children" | jq -r '.[] | [.name, .metrics] | @tsv')
    printf ']\n' >> "$tmp"
    jq -c '
      {
        available: true,
        sample_count: length,
        total_rss_kb: (map(.rss_kb // 0) | add // 0),
        total_pss_kb: (map(.pss_kb // 0) | add // 0),
        total_shared_clean_kb: (map(.shared_clean_kb // 0) | add // 0),
        total_shared_dirty_kb: (map(.shared_dirty_kb // 0) | add // 0),
        estimated_shared_savings_kb: (((map(.rss_kb // 0) | add // 0) - (map(.pss_kb // 0) | add // 0)) | if . < 0 then 0 else . end),
        children: .
      }
    ' "$tmp"
    rm -f "$tmp"
}

snapshot_dir_for() {
    local id="$1"
    printf '%s/%s' "$CH_SNAPSHOT_ROOT" "$id"
}

write_provenance() {
    local snapshot_dir="$1"
    local id="$2"
    local posture="$3"
    local pre_enrollment="$4"
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
    gpg --batch --verify "$snapshot_dir/provenance.sha256.sig" "$snapshot_dir/provenance.sha256" >/dev/null \
        || die "snapshot signature verification failed: $snapshot_dir"
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
    find "$snapshot_dir/ch-state" -type f -exec chmod 0444 {} +

    local posture="clean-base"
    if [[ "$secret_bearing" == true ]]; then
        posture="secret-bearing-sealed"
    fi
    CH_SNAPSHOT_SIGN_KEY="$sign_key" write_provenance "$snapshot_dir" "$id" "$posture" "$pre_enrollment"

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

    if (( duration_ms > CH_RESTORE_LATENCY_BUDGET_MS )); then
        die "restore latency ${duration_ms}ms exceeded budget ${CH_RESTORE_LATENCY_BUDGET_MS}ms"
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
}

cmd_fork() {
    local snapshot="" prefix="" count="" mode="ondemand"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --snapshot) snapshot="$2"; shift 2 ;;
            --prefix) prefix="$2"; shift 2 ;;
            --count) count="$2"; shift 2 ;;
            --mode) mode="$2"; shift 2 ;;
            *) die "unknown fork arg: $1" ;;
        esac
    done
    [[ -n "$snapshot" && -n "$prefix" && "$count" =~ ^[0-9]+$ && "$count" -gt 0 ]] || die "fork requires --snapshot, --prefix, --count N"
    validate_identifier "snapshot id" "$snapshot"
    validate_identifier "child prefix" "$prefix"
    [[ "$mode" == "ondemand" || "$mode" == "copy" ]] || die "--mode must be ondemand or copy"
    local snapshot_dir
    snapshot_dir="$(snapshot_dir_for "$snapshot")"
    verify_snapshot "$snapshot_dir"
    require_snapshot_agentshare_for_bootstrap "$snapshot_dir"
    local children
    CH_ACTIVE_RESTORE_SNAPSHOT="$snapshot"
    CH_ACTIVE_RESTORE_INSTANCE_ID=""
    CH_ACTIVE_RESTORE_CHILD_PREFIX="$prefix"
    children="$(backend_fork_vm "$snapshot_dir" "$prefix" "$count" "$mode")"
    local manifest="$VM_STORAGE_DIR/${prefix}-fork-manifest.json"
    local memory_sharing
    memory_sharing="$(fork_memory_sharing_json "$children")"
    cat > "$manifest" <<EOF
{
  "snapshot_id": "$(json_escape "$snapshot")",
  "child_prefix": "$(json_escape "$prefix")",
  "count": $count,
  "memory_restore_mode": "$(json_escape "$mode")",
  "children": $children,
  "memory_sharing": $memory_sharing,
  "fresh_identity_per_child": true,
  "disk_cow_per_child": true
}
EOF
    printf '{"manifest":"%s","children":%s}\n' "$(json_escape "$manifest")" "$children"
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
    local snapshot_dir pool_dir
    snapshot_dir="$(snapshot_dir_for "$snapshot")"
    verify_snapshot "$snapshot_dir"
    pool_dir="$CH_WARM_POOL_ROOT/$prefix"
    mkdir -p "$pool_dir"
    cat > "$pool_dir/pool.json" <<EOF
{
  "pool": "$(json_escape "$prefix")",
  "snapshot_id": "$(json_escape "$snapshot")",
  "target_size": $size,
  "pre_enrollment": true,
  "idle": $size,
  "handed_out": 0
}
EOF
    printf '{"pool":"%s","snapshot_id":"%s","target_size":%s,"idle":%s}\n' \
        "$(json_escape "$prefix")" "$(json_escape "$snapshot")" "$size" "$size"
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
    local snapshot idle
    snapshot="$(jq -r '.snapshot_id // empty' "$pool_file")"
    idle="$(jq -r '.idle // 0' "$pool_file")"
    [[ -n "$snapshot" ]] || die "warm pool missing snapshot_id: $pool_file"
    (( idle > 0 )) || die "warm pool $pool has no idle capacity"
    local -a restore_args=(--snapshot "$snapshot" --name "$name" --mode "$mode")
    [[ -n "$instance_id" ]] && restore_args+=(--instance-id "$instance_id")
    [[ "$wait_enrollment_ready" == true ]] && restore_args+=(--wait-enrollment-ready)
    local restore_output
    # Cold restore may include DHCP and first enrollment. A genuine warm slot
    # must still satisfy the subsecond handoff acceptance budget.
    local CH_RESTORE_READY_LATENCY_BUDGET_MS="$CH_WARM_HANDOFF_READY_LATENCY_BUDGET_MS"
    restore_output="$(cmd_restore "${restore_args[@]}")"
    local pool_tmp
    pool_tmp="$(mktemp "${pool_file}.tmp.XXXXXX")"
    jq '.idle -= 1 | .handed_out += 1' "$pool_file" > "$pool_tmp"
    mv -f "$pool_tmp" "$pool_file"
    flock -u "$pool_lock_fd"
    printf '%s\n' "$restore_output"
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
    snapshot) shift; cmd_snapshot "$@" ;;
    restore) shift; cmd_restore "$@" ;;
    fork) shift; cmd_fork "$@" ;;
    warm-init) shift; cmd_warm_init "$@" ;;
    warm-handoff) shift; cmd_warm_handoff "$@" ;;
    verify) shift; cmd_verify "$@" ;;
    -h|--help|"") usage ;;
    *) usage; die "unknown subcommand: $1" ;;
esac
