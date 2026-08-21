#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
QEMU_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

export VM_STORAGE_DIR="$TMP_ROOT/vms"
export AGENTSHARE_ROOT="$TMP_ROOT/agentshare"
export LIBVIRT_CHECKPOINT_ROOT="$TMP_ROOT/checkpoints"
export LIBVIRT_WARM_POOL_ROOT="$TMP_ROOT/warm-pools"
export IP_REGISTRY="$TMP_ROOT/ip-registry"
export CID_REGISTRY="$TMP_ROOT/cid-registry"
mkdir -p "$VM_STORAGE_DIR" "$AGENTSHARE_ROOT" "$LIBVIRT_CHECKPOINT_ROOT"

# shellcheck source=images/qemu/checkpoint-vm.sh
source "$QEMU_DIR/checkpoint-vm.sh"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); printf 'PASS: %s\n' "$1"; }
fail() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$1" >&2; }
assert_file_contains() {
    local label="$1" pattern="$2" file="$3"
    if grep -Fq -- "$pattern" "$file"; then pass "$label"; else fail "$label"; fi
}
assert_json() {
    local label="$1" expression="$2" file="$3"
    if jq -e "$expression" "$file" >/dev/null; then pass "$label"; else fail "$label"; fi
}

PERSISTENCE_PROBE="$TMP_ROOT/persistence-probe"
printf '0\n' > "$PERSISTENCE_PROBE"
virsh() {
    [[ "$1" == "dominfo" ]] || return 1
    local probe
    probe="$(<"$PERSISTENCE_PROBE")"
    probe=$((probe + 1))
    printf '%s\n' "$probe" > "$PERSISTENCE_PROBE"
    if [[ "${PERSISTENCE_ALWAYS_TRANSIENT:-false}" == "true" ]] \
        || (( probe == 1 || probe == 3 )); then
        printf 'Persistent:     no\n'
    else
        printf 'Persistent:     yes\n'
    fi
}
if _wait_domain_persistent restored-vm 6 0 2; then
    pass "persistence wait requires stable consecutive observations"
else
    fail "persistence wait requires stable consecutive observations"
fi
if [[ "$(<"$PERSISTENCE_PROBE")" == 5 ]]; then
    pass "persistence wait resets after a transient observation"
else
    fail "persistence wait resets after a transient observation"
fi
PERSISTENCE_ALWAYS_TRANSIENT=true
if _wait_domain_persistent restored-vm 3 0 2; then
    fail "persistence wait rejects a domain that remains transient"
else
    pass "persistence wait rejects a domain that remains transient"
fi
unset PERSISTENCE_ALWAYS_TRANSIENT
unset -f virsh

SAVE_TIMEOUT_PROBE="$TMP_ROOT/save-timeout-probe"
SAVE_DESTROY_PROBE="$TMP_ROOT/save-destroy-probe"
export LIBVIRT_SAVE_TIMEOUT_SECONDS=1
export LIBVIRT_SAVE_KILL_AFTER_SECONDS=1
export LIBVIRT_SAVE_CLEANUP_TIMEOUT_SECONDS=1
timeout() {
    printf '%s\n' "$*" >> "$SAVE_TIMEOUT_PROBE"
    shift 3
    if [[ "$1" == virsh && "$2" == save ]]; then
        : > "$4"
        return 124
    fi
    "$@"
}
virsh() {
    # Invoked indirectly by the timeout test double above.
    # shellcheck disable=SC2317
    case "$1" in
        domstate) printf 'running\n' ;;
        destroy) printf '%s\n' "$2" > "$SAVE_DESTROY_PROBE" ;;
        *) return 1 ;;
    esac
}
partial="$TMP_ROOT/timed-out.save"
for suffix in '' .domain.xml .virtiofs.xml .nvram .metadata.json .state.json; do
    : > "$partial$suffix"
done
save_rc=0
_bound_virsh_save timeout-vm "$partial" false 2> "$TMP_ROOT/save-timeout.stderr" || save_rc=$?
if [[ "$save_rc" == 124 ]]; then
    pass "hung virsh save returns timeout status"
else
    fail "hung virsh save returns timeout status"
fi
if compgen -G "$partial*" >/dev/null; then
    fail "timed-out save removes partial checkpoint artifacts"
else
    pass "timed-out save removes partial checkpoint artifacts"
fi
assert_file_contains "timed-out save forces a deterministic shutoff" "timeout-vm" "$SAVE_DESTROY_PROBE"
assert_file_contains "timeout diagnostic names phase and cleanup" \
    "phase=virsh-save timeout_seconds=1 cleanup=forced-shutoff" "$TMP_ROOT/save-timeout.stderr"
assert_file_contains "save timeout uses TERM with a KILL grace" \
    "--signal=TERM --kill-after=1s 1s virsh save timeout-vm $partial" "$SAVE_TIMEOUT_PROBE"
assert_file_contains "CI bounds the complete libvirt checkpoint self-test" \
    "timeout --signal=TERM --kill-after=30s 20m" "$QEMU_DIR/../../.gitea/workflows/ci.yaml"
unset -f timeout virsh

DETACH_PROBE="$TMP_ROOT/detach-probe"
printf '0\n' > "$DETACH_PROBE"
virsh() {
    case "$1" in
        dumpxml)
            if [[ "$(<"$DETACH_PROBE")" -lt 3 ]]; then
                cat <<'XML'
<domain><devices>
  <filesystem type='mount'><driver type='virtiofs'/><source dir='/share'/><target dir='agentglobal'/></filesystem>
  <filesystem type='mount'><driver type='path'/><source dir='/legacy'/><target dir='legacy'/></filesystem>
</devices></domain>
XML
            fi
            ;;
        detach-device)
            local probe
            probe="$(<"$DETACH_PROBE")"
            probe=$((probe + 1))
            printf '%s\n' "$probe" > "$DETACH_PROBE"
            (( probe >= 3 ))
            ;;
        *) return 1 ;;
    esac
}
blocks="$(_virtiofs_blocks retry-vm)"
if [[ "$blocks" == *"agentglobal"* && "$blocks" != *"legacy"* ]]; then
    pass "virtiofs discovery excludes non-virtiofs filesystems"
else
    fail "virtiofs discovery excludes non-virtiofs filesystems"
fi
if [[ "$(_detach_virtiofs retry-vm "$blocks")" == 1 && "$(<"$DETACH_PROBE")" == 3 ]]; then
    pass "virtiofs detach retries and verifies live removal"
else
    fail "virtiofs detach retries and verifies live removal"
fi
unset -f virsh

cat > "$TMP_ROOT/source.xml" <<'XML'
<domain type='kvm'>
  <name>clean-base</name>
  <uuid>aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee</uuid>
  <os><nvram>/old/base_VARS.fd</nvram></os>
  <devices>
    <disk type='file' device='disk'>
      <source file='/old/base.qcow2'/>
    </disk>
    <interface type='network'><mac address='52:54:00:00:00:01'/></interface>
    <vsock model='virtio'><cid auto='yes' address='3'/></vsock>
  </devices>
</domain>
XML
_rewrite_domain_xml "$TMP_ROOT/source.xml" "$TMP_ROOT/child.xml" clean-base 44 \
    "$TMP_ROOT/child.qcow2" "$TMP_ROOT/child_VARS.fd"
assert_file_contains "restore XML preserves saved-image name" "<name>clean-base</name>" "$TMP_ROOT/child.xml"
assert_file_contains "restore XML preserves saved-image UUID" "<uuid>aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee</uuid>" "$TMP_ROOT/child.xml"
assert_file_contains "restore XML uses fresh disk overlay" "file='$TMP_ROOT/child.qcow2'" "$TMP_ROOT/child.xml"
assert_file_contains "restore XML uses fresh NVRAM" ">$TMP_ROOT/child_VARS.fd</nvram>" "$TMP_ROOT/child.xml"
assert_file_contains "restore XML preserves saved-image MAC" "address='52:54:00:00:00:01'" "$TMP_ROOT/child.xml"
assert_file_contains "restore XML uses fresh CID" "cid auto='no' address='44'" "$TMP_ROOT/child.xml"

cat > "$TMP_ROOT/virtiofs.xml" <<'XML'
<filesystem type='mount' accessmode='passthrough'>
  <driver type='virtiofs'/><source dir='/srv/agentshare/global-ro'/><target dir='agentglobal'/>
</filesystem>
<filesystem type='mount' accessmode='passthrough'>
  <driver type='virtiofs'/><source dir='/srv/agentshare/base-inbox'/><target dir='agentinbox'/>
</filesystem>
XML
_rewrite_virtiofs_sidecar "$TMP_ROOT/virtiofs.xml" "$TMP_ROOT/child-fs.xml" "$AGENTSHARE_ROOT/child-one-inbox"
assert_file_contains "global share remains unchanged" "dir='/srv/agentshare/global-ro'" "$TMP_ROOT/child-fs.xml"
assert_file_contains "child gets isolated inbox" "dir='$AGENTSHARE_ROOT/child-one-inbox'" "$TMP_ROOT/child-fs.xml"

_guest_exec_capture() {
    printf '%s\n' '{"schema":1,"agent_inactive":true,"no_tls_identity":true,"no_bootstrap_or_oauth":true}'
}
if _prepare_clean_base clean-base >/dev/null; then pass "clean-base attestation is accepted"; else fail "clean-base attestation is accepted"; fi
_guest_exec_capture() {
    printf '%s\n' '{"schema":1,"agent_inactive":true,"no_tls_identity":false,"no_bootstrap_or_oauth":true}'
}
if (_prepare_clean_base dirty-base >/dev/null 2>&1); then fail "credential-bearing base is rejected"; else pass "credential-bearing base is rejected"; fi

export LIBVIRT_BOOTSTRAP_STDIN_PAYLOAD='{"single":{"instance_id":"018fc0a2-7777-7aaa-bbbb-ccccddddeeee","token":"unit-test-token","spiffe_id":"spiffe://sandbox.agentic.local/agent/018fc0a2-7777-7aaa-bbbb-ccccddddeeee","expires_at_unix_ms":1784319999000,"tls_dir":"/etc/agentic-sandbox/grpc-mtls","enrollment_url":"https://host.internal:8124/api/v1/bootstrap-enrollment/consume","ca_pem":"-----BEGIN CERTIFICATE-----\\ntest\\n-----END CERTIFICATE-----"}}'
_stage_restore_bootstrap child-one 018fc0a2-7777-7aaa-bbbb-ccccddddeeee \
    "$AGENTSHARE_ROOT/child-one-inbox" 192.168.122.240 52:54:00:aa:bb:cc
assert_file_contains "bootstrap drop targets fresh identity" "AGENT_INSTANCE_ID=018fc0a2-7777-7aaa-bbbb-ccccddddeeee" "$AGENTSHARE_ROOT/child-one-inbox/restore-bootstrap.env"
assert_file_contains "bootstrap drop carries one-time token" "AGENT_BOOTSTRAP_TOKEN=unit-test-token" "$AGENTSHARE_ROOT/child-one-inbox/restore-bootstrap.env"
if [[ "$(stat -c '%a' "$AGENTSHARE_ROOT/child-one-inbox/restore-bootstrap.env")" == 600 ]]; then pass "bootstrap drop is mode 0600"; else fail "bootstrap drop is mode 0600"; fi

create_test_checkpoint() {
    local checkpoint_id="$1" checkpoint_dir="$LIBVIRT_CHECKPOINT_ROOT/$1"
    mkdir -p "$checkpoint_dir"
    local suffix
    for suffix in '' .domain.xml .virtiofs.xml .nvram; do
        printf 'artifact-%s\n' "$suffix" > "$checkpoint_dir/checkpoint.save$suffix"
    done
    jq -n --arg id "$checkpoint_id" --arg source_vm "$checkpoint_id" \
        '{schema:1,backend:"libvirt",checkpoint_id:$id,source_vm:$source_vm,source_disk:"/base.qcow2",pre_enrollment:true}' \
        > "$checkpoint_dir/checkpoint.save.metadata.json"
    jq -n --arg id "$checkpoint_id" '{schema:1,backend:"libvirt",checkpoint_id:$id,status:"available"}' \
        > "$checkpoint_dir/checkpoint.save.state.json"
}
create_test_checkpoint qemu-clean-a
create_test_checkpoint qemu-clean-b
cmd_warm_init --checkpoint qemu-clean-a --checkpoint qemu-clean-b --pool qemu-pool > "$TMP_ROOT/warm-init.json"
assert_json "warm pool records two available slots" '[.slots[] | select(.status == "available")] | length == 2' "$LIBVIRT_WARM_POOL_ROOT/qemu-pool/state.json"
assert_json "warm-init reports prebooted capacity" '.prebooted == true and .available == 2' "$TMP_ROOT/warm-init.json"
assert_json "slot A checkpoint is reserved" '.status == "reserved" and .pool == "qemu-pool"' "$LIBVIRT_CHECKPOINT_ROOT/qemu-clean-a/checkpoint.save.state.json"
assert_json "slot B checkpoint is reserved" '.status == "reserved" and .pool == "qemu-pool"' "$LIBVIRT_CHECKPOINT_ROOT/qemu-clean-b/checkpoint.save.state.json"

cmd_restore() {
    local in="$1"; shift
    printf '%s\n' "$in $*" >> "$TMP_ROOT/restore-calls"
    printf '%s\n' '{"name":"warm-child","enroll_on_restore":true,"bootstrap_token_issued":true,"enrollment_ready":true,"duration_ms":2800}'
}
cmd_warm_handoff --pool qemu-pool --instance-id 018fc0a2-7777-7aaa-bbbb-ccccddddee01 > "$TMP_ROOT/handoff-1.json"
cmd_warm_handoff --pool qemu-pool --instance-id 018fc0a2-7777-7aaa-bbbb-ccccddddee02 > "$TMP_ROOT/handoff-2.json"
assert_json "first handoff consumes slot one" '.claimed_slot == "slot-1"' "$TMP_ROOT/handoff-1.json"
assert_json "second handoff consumes slot two" '.claimed_slot == "slot-2"' "$TMP_ROOT/handoff-2.json"
assert_json "pool records both slots consumed" '[.slots[] | select(.status == "consumed")] | length == 2' "$LIBVIRT_WARM_POOL_ROOT/qemu-pool/state.json"
assert_json "slot A checkpoint is consumed" '.status == "consumed"' "$LIBVIRT_CHECKPOINT_ROOT/qemu-clean-a/checkpoint.save.state.json"
assert_json "slot B checkpoint is consumed" '.status == "consumed"' "$LIBVIRT_CHECKPOINT_ROOT/qemu-clean-b/checkpoint.save.state.json"
if (cmd_warm_handoff --pool qemu-pool --instance-id 018fc0a2-7777-7aaa-bbbb-ccccddddee03 >/dev/null 2>&1); then
    fail "exhausted pool rejects another handoff"
else
    pass "exhausted pool rejects another handoff"
fi

printf '\nResults: %s passed, %s failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
