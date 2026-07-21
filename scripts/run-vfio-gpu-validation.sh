#!/usr/bin/env bash
# Manual, serialized two-tenant physical-GPU validation for a dedicated runner.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: run-vfio-gpu-validation.sh --config PATH --run-id ID --artifact-dir PATH

The command provisions and destroys two exact Cloud Hypervisor VM names. It is
intended only for the restricted, manually dispatched vfio-gpu runner lane.
EOF
}

CONFIG=""
RUN_ID=""
ARTIFACT_DIR=""
CI_REPOSITORY="${GITEA_REPOSITORY:-}"
CI_REF="${GITEA_REF:-}"
CI_ACTOR="${GITEA_ACTOR:-}"
CI_EVENT="${GITEA_EVENT_NAME:-}"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --config) CONFIG="${2:?--config requires a path}"; shift 2 ;;
        --run-id) RUN_ID="${2:?--run-id requires a value}"; shift 2 ;;
        --artifact-dir) ARTIFACT_DIR="${2:?--artifact-dir requires a path}"; shift 2 ;;
        --repository) CI_REPOSITORY="${2:?--repository requires a value}"; shift 2 ;;
        --ref) CI_REF="${2:?--ref requires a value}"; shift 2 ;;
        --actor) CI_ACTOR="${2:?--actor requires a value}"; shift 2 ;;
        --event) CI_EVENT="${2:?--event requires a value}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

[[ -f "$CONFIG" ]] || { echo "Runner config missing: $CONFIG" >&2; exit 1; }
[[ "$RUN_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] \
    || { echo "Unsafe run id" >&2; exit 2; }
[[ "$ARTIFACT_DIR" == "/var/tmp/agentic-vfio-evidence/run-$RUN_ID" ]] \
    || { echo "Artifact path must exactly match the validated run id" >&2; exit 2; }
[[ "$EUID" -eq 0 ]] || { echo "Physical VFIO validation must run through the reviewed sudo entrypoint" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
GUARD="$SCRIPT_DIR/vfio-gpu-runner-guard.sh"
VERIFY_GPU="$PROJECT_ROOT/images/qemu/tests/verify-ch-gpu-passthrough.sh"
DESTROY_VM="$SCRIPT_DIR/destroy-vm.sh"

jq -e '
  (.validation.base_image | type == "string" and startswith("/") and length > 1) and
  (.validation.ssh_key | type == "string" and startswith("/") and length > 1) and
  (.validation.ssh_user | type == "string" and test("^[A-Za-z_][A-Za-z0-9._-]*$")) and
  (.validation.cpus | type == "number" and . >= 1) and
  (.validation.memory | type == "string" and test("^[1-9][0-9]*[MG]$")) and
  (.validation.disk | type == "string" and test("^[1-9][0-9]*G$")) and
  (.validation.network | type == "string" and length > 0) and
  (.validation.guest_functionality_command | type == "string" and length > 0) and
  (.validation.residue_tool | type == "string" and startswith("/") and length > 1)
' "$CONFIG" >/dev/null || { echo "Runner validation configuration is incomplete" >&2; exit 1; }
config_mode="$(stat -c '%a' "$CONFIG")"
[[ "$(stat -c '%U' "$CONFIG")" == "root" && $((8#$config_mode & 8#022)) -eq 0 ]] \
    || { echo "Runner inventory must be root-owned and not group/other writable" >&2; exit 1; }

expected_repo="$(jq -r '.repository' "$CONFIG")"
expected_ref="$(jq -r '.allowed_ref' "$CONFIG")"
[[ "$CI_REPOSITORY" == "$expected_repo" ]] \
    || { echo "Refusing repository context: ${CI_REPOSITORY:-unset}" >&2; exit 1; }
[[ "$CI_REF" == "$expected_ref" ]] \
    || { echo "Refusing git ref: ${CI_REF:-unset}" >&2; exit 1; }
[[ "$CI_EVENT" == "workflow_dispatch" ]] \
    || { echo "VFIO GPU validation is manual-dispatch only" >&2; exit 1; }
jq -e --arg actor "$CI_ACTOR" '.allowed_actors | index($actor) != null' "$CONFIG" >/dev/null \
    || { echo "Actor is not allowlisted for the VFIO GPU lane" >&2; exit 1; }

lock_file="$(jq -r '.lock_file' "$CONFIG")"
install -d -m 0755 "$(dirname "$lock_file")"
exec 9>"$lock_file"
flock -n 9 || { echo "The approved test IOMMU group is already reserved" >&2; exit 1; }

install -d -m 0755 "$ARTIFACT_DIR"
"$GUARD" preflight --config "$CONFIG" > "$ARTIFACT_DIR/preflight.json"
"$GUARD" inventory --config "$CONFIG" > "$ARTIFACT_DIR/inventory-before.json"
vm_a="vfio-${RUN_ID}-a"
vm_b="vfio-${RUN_ID}-b"
test_bdf="$(jq -r '.test_gpu.bdf | ascii_downcase' "$CONFIG")"
base_image="$(jq -r '.validation.base_image' "$CONFIG")"
ssh_key="$(jq -r '.validation.ssh_key' "$CONFIG")"
ssh_user="$(jq -r '.validation.ssh_user' "$CONFIG")"
cpus="$(jq -r '.validation.cpus' "$CONFIG")"
memory="$(jq -r '.validation.memory' "$CONFIG")"
disk="$(jq -r '.validation.disk' "$CONFIG")"
network="$(jq -r '.validation.network' "$CONFIG")"
functionality_command="$(jq -r '.validation.guest_functionality_command' "$CONFIG")"
residue_tool="$(jq -r '.validation.residue_tool' "$CONFIG")"

[[ -r "$base_image" && -r "$ssh_key" && -x "$residue_tool" ]] \
    || { echo "Base image, SSH key, or residue tool is unavailable" >&2; exit 1; }
[[ "$(stat -c '%U' "$ssh_key")" == "root" && "$(stat -c '%a' "$ssh_key")" =~ ^(400|600)$ ]] \
    || { echo "SSH key must be owner-only" >&2; exit 1; }
residue_mode="$(stat -c '%a' "$residue_tool")"
if [[ "$(stat -c '%U' "$residue_tool")" != "root" ]] || (( (8#$residue_mode & 8#022) != 0 )); then
    echo "Residue tool must not be group/other writable" >&2
    exit 1
fi

loadout="$ARTIFACT_DIR/vfio-validation-loadout.yaml"
cat > "$loadout" <<EOF
apiVersion: loadout/v1
kind: loadout
metadata:
  name: vfio-hardware-validation
  description: Restricted physical GPU validation lane
  labels:
    category: task-focused
resources:
  cpus: $cpus
  memory: $memory
  disk: $disk
  gpu:
    enabled: true
    device: "$test_bdf"
    driver: vfio-pci
network:
  mode: full
packages:
  - pciutils
EOF

declare -A created=( ["$vm_a"]=false ["$vm_b"]=false )
cleanup_failed=0
cleanup() {
    local rc=$? vm
    trap - EXIT INT TERM
    for vm in "$vm_b" "$vm_a"; do
        if [[ "${created[$vm]}" == true && -d "/var/lib/agentic-sandbox/vms/$vm" ]]; then
            if ! AGENTIC_BACKEND=cloud-hypervisor "$DESTROY_VM" "$vm" --force \
                >"$ARTIFACT_DIR/${vm}-managed-destroy.log" 2>&1; then
                cleanup_failed=1
            fi
        fi
    done
    if (( cleanup_failed != 0 )); then
        echo "Managed teardown failed; VFIO claim/recovery state was intentionally retained" >&2
        exit 1
    fi
    exit "$rc"
}
trap cleanup EXIT INT TERM

ssh_args=(-o BatchMode=yes -o StrictHostKeyChecking=accept-new \
    -o "UserKnownHostsFile=$ARTIFACT_DIR/known_hosts" -o ConnectTimeout=15 -i "$ssh_key")

service_driver_expected="$(jq -r '.service_gpu.native_driver' "$CONFIG")"
assert_service_gpu() {
    local stage="$1" actual
    actual="$("$GUARD" inventory --config "$CONFIG" | jq -r '.service_gpu.driver')"
    [[ "$actual" == "$service_driver_expected" ]] \
        || { echo "Service GPU was not preserved during $stage" >&2; return 1; }
}

guest_ip() {
    sed -n 's/^IP_ADDRESS=//p' "/var/lib/agentic-sandbox/vms/$1/cloud-hypervisor/vm.env" | head -n1
}

provision_tenant() {
    local vm="$1" phase="$2" ip remote_tool="/tmp/agentic-vfio-residue-probe"
    local -a verify_args=()
    AGENTIC_BACKEND=cloud-hypervisor "$PROJECT_ROOT/images/qemu/provision-vm.sh" "$vm" \
        --base "$base_image" --loadout "$loadout" --ssh-key "$ssh_key" \
        --network "$network" --wait > "$ARTIFACT_DIR/${vm}-provision.log"
    created[$vm]=true
    assert_service_gpu "$vm provisioning"
    ip="$(guest_ip "$vm")"
    [[ -n "$ip" ]] || { echo "Guest IP missing for $vm" >&2; return 1; }
    if [[ "$phase" == "probe-before-write" ]]; then
        verify_args+=(--skip-driver-probe)
    fi
    "$VERIFY_GPU" "$vm" --host "$ip" --user "$ssh_user" --key "$ssh_key" \
        --evidence "$ARTIFACT_DIR/${vm}-gpu-validation.json" "${verify_args[@]}"
    scp -q "${ssh_args[@]}" "$residue_tool" "$ssh_user@$ip:$remote_tool"
    # shellcheck disable=SC2029 # Both remote path and phase are fixed/validated locally.
    ssh "${ssh_args[@]}" "$ssh_user@$ip" chmod 700 "$remote_tool"
    # shellcheck disable=SC2029 # Both remote path and phase are fixed/validated locally.
    ssh "${ssh_args[@]}" "$ssh_user@$ip" "$remote_tool" "$phase" \
        > "$ARTIFACT_DIR/${vm}-residue.json"
    jq -e --arg phase "$phase" \
        '.result == "pass" and ($phase != "probe-before-write" or .prewrite == true)' \
        "$ARTIFACT_DIR/${vm}-residue.json" >/dev/null \
        || { echo "Residue tool failed for $vm phase $phase" >&2; return 1; }
    # The driver-level probe is deliberately after tenant B's pre-write check.
    # shellcheck disable=SC2029 # Root-owned inventory intentionally defines the remote probe.
    ssh "${ssh_args[@]}" "$ssh_user@$ip" "$functionality_command" \
        > "$ARTIFACT_DIR/${vm}-functionality.txt"
}

destroy_and_verify() {
    local vm="$1"
    AGENTIC_BACKEND=cloud-hypervisor "$DESTROY_VM" "$vm" --force \
        > "$ARTIFACT_DIR/${vm}-managed-destroy.log"
    created[$vm]=false
    "$GUARD" postflight --config "$CONFIG" --vm "$vm" \
        > "$ARTIFACT_DIR/${vm}-postflight.json"
}

provision_tenant "$vm_a" fill
destroy_and_verify "$vm_a"
provision_tenant "$vm_b" probe-before-write
destroy_and_verify "$vm_b"
"$GUARD" inventory --config "$CONFIG" > "$ARTIFACT_DIR/inventory-after.json"

jq -n \
    --arg checked_at "$(date --utc +%Y-%m-%dT%H:%M:%SZ)" \
    --arg run_id "$RUN_ID" --arg tenant_a "$vm_a" --arg tenant_b "$vm_b" \
    '{schema:1,checked_at:$checked_at,run_id:$run_id,tenants:[$tenant_a,$tenant_b],guest_gpu_use:true,managed_teardown:true,cross_tenant_residue_test:true,service_gpu_preserved:true,result:"pass"}' \
    > "$ARTIFACT_DIR/result.json"
(
    cd "$ARTIFACT_DIR"
    find . -maxdepth 1 -type f ! -name evidence.sha256 -print0 | sort -z | xargs -0 sha256sum
) > "$ARTIFACT_DIR/evidence.sha256"

trap - EXIT INT TERM
echo "PASS: dual-tenant VFIO GPU validation completed; evidence: $ARTIFACT_DIR"
