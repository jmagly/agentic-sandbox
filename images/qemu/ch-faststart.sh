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
  $0 snapshot --vm NAME --id ID --pre-enrollment [--sign-key KEY]
  $0 snapshot --vm NAME --id ID --secret-bearing --seal-key KEY [--sign-key KEY]
  $0 restore  --snapshot ID --name NAME [--mode ondemand|copy] [--instance-id ID]
  $0 fork     --snapshot ID --prefix NAME --count N [--mode ondemand|copy]
  $0 warm-init --snapshot ID --size N --prefix NAME
  $0 warm-handoff --pool NAME --name NAME [--mode ondemand|copy]
  $0 verify   --snapshot ID
EOF
}

die() {
    log_error "$*"
    exit 1
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
    if [[ -z "${CH_CHILD_BOOTSTRAP_ENVELOPES:-}" ]]; then
        return 0
    fi
    command -v jq >/dev/null 2>&1 || die "jq is required for CH child bootstrap enrollment metadata"
    printf '%s' "$CH_CHILD_BOOTSTRAP_ENVELOPES" | jq -c --arg name "$name" '.[$name] // empty'
}

write_enroll_on_restore_metadata() {
    local name="$1" instance_id="$2" snapshot="$3" cid="$4"
    local vm_dir="$VM_STORAGE_DIR/$name"
    local bootstrap_json token spiffe expires tls_dir enrollment_url bootstrap_env_path token_issued=false
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
    fi
    if [[ -n "$token" || -n "$spiffe" ]]; then
        [[ -n "$token" && -n "$spiffe" ]] || die "bootstrap enrollment requires both token and spiffe_id for $name"
        bootstrap_env_path="$vm_dir/restore-bootstrap.env"
        cat > "$bootstrap_env_path" <<EOF
AGENT_TRANSPORT=auto
AGENT_BOOTSTRAP_TOKEN=$token
AGENT_BOOTSTRAP_SPIFFE_ID=$spiffe
EOF
        if [[ -n "$expires" ]]; then
            printf 'AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=%s\n' "$expires" >> "$bootstrap_env_path"
        fi
        if [[ -n "$tls_dir" ]]; then
            printf 'AGENT_BOOTSTRAP_TLS_DIR=%s\n' "$tls_dir" >> "$bootstrap_env_path"
        fi
        if [[ -n "$enrollment_url" ]]; then
            printf 'AGENT_BOOTSTRAP_ENROLLMENT_URL=%s\n' "$enrollment_url" >> "$bootstrap_env_path"
        fi
        chmod 600 "$bootstrap_env_path"
        token_issued=true
    fi

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
  "bootstrap_env_path": "$(json_escape "${bootstrap_env_path:-}")",
  "bootstrap_env_mode": "$(if [[ -n "${bootstrap_env_path:-}" ]]; then stat -c '%a' "$bootstrap_env_path"; fi)"
}
EOF
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
    cat > "$snapshot_dir/secret-posture.json" <<EOF
{
  "snapshot_id": "$(json_escape "$id")",
  "pre_enrollment": $pre_enrollment,
  "posture": "$(json_escape "$posture")",
  "base_contains_tenant_secrets": false,
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
    if [[ -f "$snapshot_dir/provenance.sha256.sig" ]]; then
        gpg --batch --verify "$snapshot_dir/provenance.sha256.sig" "$snapshot_dir/provenance.sha256" >/dev/null
    fi
    if ! grep -q '"pre_enrollment"[[:space:]]*:[[:space:]]*true' "$snapshot_dir/secret-posture.json" 2>/dev/null; then
        die "refusing to restore non-clean-base snapshot without an explicit unseal workflow: $snapshot_dir"
    fi
}

cmd_snapshot() {
    local vm="" id="" pre_enrollment=false secret_bearing=false seal_key=""
    CH_SNAPSHOT_SIGN_KEY=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --vm) vm="$2"; shift 2 ;;
            --id) id="$2"; shift 2 ;;
            --pre-enrollment) pre_enrollment=true; shift ;;
            --secret-bearing) secret_bearing=true; shift ;;
            --seal-key) seal_key="$2"; shift 2 ;;
            --sign-key) CH_SNAPSHOT_SIGN_KEY="$2"; shift 2 ;;
            *) die "unknown snapshot arg: $1" ;;
        esac
    done
    [[ -n "$vm" && -n "$id" ]] || die "snapshot requires --vm and --id"
    if [[ "$pre_enrollment" != true && "$secret_bearing" != true ]]; then
        die "snapshot requires --pre-enrollment clean base or --secret-bearing --seal-key"
    fi
    if [[ "$secret_bearing" == true && -z "$seal_key" ]]; then
        die "secret-bearing CH snapshots must be sealed with --seal-key"
    fi

    local snapshot_dir
    snapshot_dir="$(snapshot_dir_for "$id")"
    mkdir -p "$snapshot_dir"
    backend_snapshot_vm "$vm" "$snapshot_dir" "file://$snapshot_dir/ch-state"

    local posture="clean-base"
    if [[ "$secret_bearing" == true ]]; then
        posture="secret-bearing-sealed"
    fi
    write_provenance "$snapshot_dir" "$id" "$posture" "$pre_enrollment"

    if [[ "$secret_bearing" == true ]]; then
        "$SCRIPT_DIR/snapshot-seal.sh" seal --key "$seal_key" --out "$snapshot_dir/sealed-snapshot.gpg" \
            "$snapshot_dir/backend-metadata.json" "$snapshot_dir/source-vm.env" \
            "$snapshot_dir/provenance.sha256" "$snapshot_dir/secret-posture.json"
        chmod 700 "$snapshot_dir"
    fi

    log_success "snapshot captured: $snapshot_dir"
    printf '{"snapshot_id":"%s","snapshot_dir":"%s","pre_enrollment":%s,"posture":"%s"}\n' \
        "$(json_escape "$id")" "$(json_escape "$snapshot_dir")" "$pre_enrollment" "$(json_escape "$posture")"
}

cmd_restore() {
    local snapshot="" name="" mode="ondemand" instance_id=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --snapshot) snapshot="$2"; shift 2 ;;
            --name) name="$2"; shift 2 ;;
            --mode) mode="$2"; shift 2 ;;
            --instance-id) instance_id="$2"; shift 2 ;;
            *) die "unknown restore arg: $1" ;;
        esac
    done
    [[ -n "$snapshot" && -n "$name" ]] || die "restore requires --snapshot and --name"
    [[ "$mode" == "ondemand" || "$mode" == "copy" ]] || die "--mode must be ondemand or copy"
    [[ -n "$instance_id" ]] || instance_id="$name"
    local snapshot_dir
    snapshot_dir="$(snapshot_dir_for "$snapshot")"
    verify_snapshot "$snapshot_dir"

    local cid child_disk start_ms end_ms metrics_path duration_ms metrics_tmp
    cid="$(allocate_cid_for_vm "$name" "$instance_id")"
    child_disk="$VM_STORAGE_DIR/$name/$name.qcow2"
    start_ms="$(ms_now)"
    metrics_tmp="$(mktemp)"
    backend_restore_vm "$name" "$snapshot_dir" "$child_disk" "$cid" "$mode" true > "$metrics_tmp"
    metrics_path="$(cat "$metrics_tmp")"
    rm -f "$metrics_tmp"
    end_ms="$(ms_now)"
    duration_ms=$((end_ms - start_ms))

    write_enroll_on_restore_metadata "$name" "$instance_id" "$snapshot" "$cid"
    if (( duration_ms > CH_RESTORE_LATENCY_BUDGET_MS )); then
        die "restore latency ${duration_ms}ms exceeded budget ${CH_RESTORE_LATENCY_BUDGET_MS}ms"
    fi
    local bootstrap_issued=false bootstrap_spiffe=""
    if [[ -f "$VM_STORAGE_DIR/$name/restore-bootstrap.env" ]]; then
        bootstrap_issued=true
        bootstrap_spiffe="$(sed -n 's/^AGENT_BOOTSTRAP_SPIFFE_ID=//p' "$VM_STORAGE_DIR/$name/restore-bootstrap.env" | head -n1)"
    fi
    printf '{"name":"%s","snapshot_id":"%s","vsock_cid":%s,"disk":"%s","metrics":"%s","duration_ms":%s,"enroll_on_restore":true,"bootstrap_token_issued":%s,"bootstrap_spiffe_id":"%s"}\n' \
        "$(json_escape "$name")" "$(json_escape "$snapshot")" "$cid" "$(json_escape "$child_disk")" "$(json_escape "$metrics_path")" "$duration_ms" "$bootstrap_issued" "$(json_escape "$bootstrap_spiffe")"
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
    [[ "$mode" == "ondemand" || "$mode" == "copy" ]] || die "--mode must be ondemand or copy"
    local snapshot_dir
    snapshot_dir="$(snapshot_dir_for "$snapshot")"
    verify_snapshot "$snapshot_dir"
    local children
    children="$(backend_fork_vm "$snapshot_dir" "$prefix" "$count" "$mode")"
    if [[ -n "${CH_CHILD_BOOTSTRAP_ENVELOPES:-}" ]]; then
        command -v jq >/dev/null 2>&1 || die "jq is required for CH fork bootstrap enrollment metadata"
        while IFS=$'\t' read -r child_name child_cid; do
            [[ -n "$child_name" && -n "$child_cid" ]] || continue
            write_enroll_on_restore_metadata "$child_name" "$child_name" "$snapshot" "$child_cid"
        done < <(printf '%s' "$children" | jq -r '.[] | [.name, .vsock_cid] | @tsv')
    fi
    local manifest="$VM_STORAGE_DIR/${prefix}-fork-manifest.json"
    cat > "$manifest" <<EOF
{
  "snapshot_id": "$(json_escape "$snapshot")",
  "child_prefix": "$(json_escape "$prefix")",
  "count": $count,
  "memory_restore_mode": "$(json_escape "$mode")",
  "children": $children,
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
    local pool="" name="" mode="ondemand" instance_id=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --pool) pool="$2"; shift 2 ;;
            --name) name="$2"; shift 2 ;;
            --mode) mode="$2"; shift 2 ;;
            --instance-id) instance_id="$2"; shift 2 ;;
            *) die "unknown warm-handoff arg: $1" ;;
        esac
    done
    [[ -n "$pool" && -n "$name" ]] || die "warm-handoff requires --pool and --name"
    local pool_file="$CH_WARM_POOL_ROOT/$pool/pool.json"
    [[ -f "$pool_file" ]] || die "warm pool not found: $pool"
    local snapshot
    snapshot="$(sed -n 's/.*"snapshot_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$pool_file" | head -n1)"
    [[ -n "$snapshot" ]] || die "warm pool missing snapshot_id: $pool_file"
    if [[ -n "$instance_id" ]]; then
        cmd_restore --snapshot "$snapshot" --name "$name" --mode "$mode" --instance-id "$instance_id"
    else
        cmd_restore --snapshot "$snapshot" --name "$name" --mode "$mode"
    fi
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
    verify_snapshot "$(snapshot_dir_for "$snapshot")"
    printf '{"snapshot_id":"%s","verified":true}\n' "$(json_escape "$snapshot")"
}

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
