#!/usr/bin/env bash
# Regression tests for VSock CID allocation, libvirt XML emission, and registry
# cleanup on destroy/reap paths.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
QEMU_DIR="$ROOT_DIR/images/qemu"

PASS=0
FAIL=0
ERRORS=()

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); ERRORS+=("$1"); }

assert_eq() {
    local label="$1"
    local expected="$2"
    local actual="$3"
    if [[ "$actual" == "$expected" ]]; then
        pass "$label"
    else
        fail "$label (expected $expected got $actual)"
    fi
}

assert_contains() {
    local label="$1"
    local needle="$2"
    local file="$3"
    if grep -qF -- "$needle" "$file"; then
        pass "$label"
    else
        fail "$label (expected to find: $needle)"
    fi
}

assert_not_contains() {
    local label="$1"
    local needle="$2"
    local file="$3"
    if ! grep -qF -- "$needle" "$file"; then
        pass "$label"
    else
        fail "$label (expected not to find: $needle)"
    fi
}

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT

# Minimal logging helpers expected by sourced libraries.
log_error() {
    echo "error: $*" >&2
}
log_info() {
    echo "info: $*"
}

echo "=== Setup: load network helpers and isolate registries ==="
VM_STORAGE_DIR="$TMP_ROOT/vms"
mkdir -p "$VM_STORAGE_DIR"

IP_REGISTRY="$TMP_ROOT/.ip-registry"
CID_REGISTRY="$TMP_ROOT/.vsock-cid-registry"
IP_BASE="192.168.122"
IP_START="201"
IP_END="254"

export VM_STORAGE_DIR IP_REGISTRY CID_REGISTRY IP_BASE IP_START IP_END

source "$QEMU_DIR/lib/network.sh"
source "$QEMU_DIR/provision-vm.sh"

write_vm_info() {
    local vm="$1"
    local instance_id="$2"
    local cid="${3:-}"
    mkdir -p "$VM_STORAGE_DIR/$vm"
    cat > "$VM_STORAGE_DIR/$vm/vm-info.json" <<EOF
{
  "name": "$vm",
  "instance_id": "$instance_id",
  "vsock_cid": "$cid"
}
EOF
}

echo ""
echo "=== Test: guest VSock target CID uses host CID, not allocated guest CID ==="
unset AGENTIC_GRPC_VSOCK_HOST_CID AGENT_GRPC_VSOCK_HOST_CID
assert_eq "guest vsock target defaults to VMADDR_CID_HOST" "2" "$(guest_vsock_host_cid)"
AGENTIC_GRPC_VSOCK_HOST_CID=7
export AGENTIC_GRPC_VSOCK_HOST_CID
assert_eq "guest vsock target honors explicit host CID override" "7" "$(guest_vsock_host_cid)"
AGENTIC_GRPC_VSOCK_HOST_CID=0
export AGENTIC_GRPC_VSOCK_HOST_CID
set +e
guest_vsock_host_cid >/tmp/agentic-vsock-host-cid-fail.log 2>&1
guest_host_cid_status=$?
set -e
if [[ "$guest_host_cid_status" == "0" ]]; then
    fail "guest vsock target rejects invalid host CID"
else
    pass "guest vsock target rejects invalid host CID"
fi
unset AGENTIC_GRPC_VSOCK_HOST_CID AGENT_GRPC_VSOCK_HOST_CID

echo ""
echo "=== Test: allocate/reuse/reclaim VSock CID registry entries ==="

: > "$CID_REGISTRY"
CID_START=3
CID_END=4
export CID_START CID_END
AGENT01_ID="018fb9f1-0001-7000-8000-000000000001"
AGENT02_ID="018fb9f1-0002-7000-8000-000000000002"
AGENT03_ID="018fb9f1-0003-7000-8000-000000000003"
CUSTOM_ID="018fb9f1-0004-7000-8000-000000000004"
write_vm_info "agent-01" "$AGENT01_ID"
write_vm_info "agent-02" "$AGENT02_ID"
write_vm_info "agent-03" "$AGENT03_ID"
write_vm_info "custom-name" "$CUSTOM_ID"

allocated=$(allocate_cid_for_vm "agent-01" "$AGENT01_ID")
assert_eq "agent-01 receives first CID in range" "3" "$allocated"

allocated=$(allocate_cid_for_vm "agent-02" "$AGENT02_ID")
assert_eq "agent-02 receives next CID in range" "4" "$allocated"

allocated=$(allocate_cid_for_vm "agent-02" "$AGENT02_ID")
assert_eq "agent-02 preserves existing CID" "4" "$allocated"

set +e
allocate_cid_for_vm "agent-03" "$AGENT03_ID" >/tmp/agentic-cid-fail.log 2>&1
allocate_status=$?
set -e
if [[ "$allocate_status" == "0" ]]; then
    fail "agent-03 fails cleanly when CID range exhausted"
else
    pass "agent-03 fails cleanly when CID range exhausted"
fi

remove_cid_allocation "agent-02"
allocated=$(allocate_cid_for_vm "agent-03" "$AGENT03_ID")
assert_eq "agent-03 reclaims freed CID" "4" "$allocated"

: > "$CID_REGISTRY"
echo "malformed-line" >> "$CID_REGISTRY"
printf '5=%s\n' "$AGENT01_ID" >> "$CID_REGISTRY"
CID_START=5
CID_END=6
export CID_START CID_END
allocated=$(allocate_cid_for_vm "custom-name" "$CUSTOM_ID")
assert_eq "custom name gets first-fit CID despite malformed lines" "6" "$allocated"

assert_eq "get_vm_allocated_cid returns deterministic row" "5" "$(get_vm_allocated_cid 'agent-01')"
assert_eq "get_vm_allocated_cid supports custom-name rows" "6" "$(get_vm_allocated_cid 'custom-name')"
printf 'legacy-vm=7\n' >> "$CID_REGISTRY"
assert_eq "get_vm_allocated_cid still supports legacy vm=cid rows" "7" "$(get_vm_allocated_cid 'legacy-vm')"

echo ""
echo "=== Test: stale non-writable CID registry files are repaired ==="
readonly_reg="$TMP_ROOT/.vsock-cid-registry-readonly"
: > "$readonly_reg"
: > "${readonly_reg}.lock"
chmod 0400 "$readonly_reg" "${readonly_reg}.lock"
saved_cid_registry="$CID_REGISTRY"; saved_start="$CID_START"; saved_end="$CID_END"
CID_REGISTRY="$readonly_reg"; CID_START=40; CID_END=41
export CID_REGISTRY CID_START CID_END
READONLY_ID="018fb9f1-0040-7000-8000-000000000040"
write_vm_info "readonly-vm" "$READONLY_ID"
allocated=$(allocate_cid_for_vm "readonly-vm" "$READONLY_ID")
assert_eq "readonly registry allocation succeeds after mode repair" "40" "$allocated"
assert_contains "readonly registry row recorded after mode repair" "40=$READONLY_ID" "$CID_REGISTRY"
if [[ -w "${readonly_reg}.lock" ]]; then
    pass "readonly lockfile is writable after mode repair"
else
    fail "readonly lockfile is writable after mode repair"
fi
CID_REGISTRY="$saved_cid_registry"; CID_START="$saved_start"; CID_END="$saved_end"
export CID_REGISTRY CID_START CID_END

echo ""
echo "=== Test: concurrent allocation is serialized (no duplicate/lost CIDs) ==="
# #581: parallel provisioners must never claim the same CID or clobber a racing
# append. Fan out N concurrent allocations against a shared registry and assert
# every allocation is unique.
saved_cid_registry="$CID_REGISTRY"; saved_start="$CID_START"; saved_end="$CID_END"
conc_reg="$TMP_ROOT/.vsock-cid-registry-conc"
: > "$conc_reg"; rm -f "${conc_reg}.lock"
CID_REGISTRY="$conc_reg"; CID_START=3; CID_END=4096
export CID_REGISTRY CID_START CID_END
conc_out="$TMP_ROOT/conc-out"; mkdir -p "$conc_out"
conc_pids=()
for i in $(seq 1 16); do
    instance_id=$(printf '018fb9f1-1%03d-7000-8000-000000000%03d' "$i" "$i")
    write_vm_info "conc-vm-$i" "$instance_id"
    ( allocate_cid_for_vm "conc-vm-$i" "$instance_id" > "$conc_out/$i" 2>/dev/null ) &
    conc_pids+=("$!")
done
for p in "${conc_pids[@]}"; do wait "$p"; done
mapfile -t conc_cids < <(cat "$conc_out"/* 2>/dev/null | grep -E '^[0-9]+$')
conc_unique=$(printf '%s\n' "${conc_cids[@]}" | sort -nu | grep -c . || true)
assert_eq "16 concurrent allocations returned 16 unique CIDs" "16" "$conc_unique"
assert_eq "16 concurrent allocations recorded 16 canonical registry rows" "16" "$(grep -cE '^[0-9]+=018fb9f1-1[0-9]{3}-7000-8000-000000000[0-9]{3}$' "$conc_reg")"
assert_eq "registry holds 16 distinct CID values" "16" "$(awk -F= '{print $1}' "$conc_reg" | sort -nu | grep -c .)"
# Restore registry globals for the remaining single-threaded tests.
CID_REGISTRY="$saved_cid_registry"; CID_START="$saved_start"; CID_END="$saved_end"
export CID_REGISTRY CID_START CID_END

echo ""
echo "=== Test: libvirt backend emits or omits <vsock> correctly ==="

mkdir -p "$TMP_ROOT/fakebin"
cat > "$TMP_ROOT/fakebin/virsh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "define" ]]; then
  exit 0
fi
exit 0
EOF
chmod 0755 "$TMP_ROOT/fakebin/virsh"

export PATH="$TMP_ROOT/fakebin:$PATH"
export AGENTIC_VM_FIRMWARE="bios"

source "$QEMU_DIR/backends/libvirt.sh"

base_disk="$TMP_ROOT/vm.qcow2"
seed_iso="$TMP_ROOT/cloud-init.iso"
touch "$base_disk" "$seed_iso"

xml_with_vsock=$(_backend_libvirt_create_vm \
    "agent-01" "$base_disk" "$seed_iso" \
    2 4096 "default" "52:54:00:12:34:56" false "" "" \
    4096 400 500 655000000 262000000 \
    "" "" "99")
assert_contains "XML includes vsock block when CID supplied" "    <vsock model='virtio'>" "$xml_with_vsock"
assert_contains "XML includes CID value" "<cid auto='no' address='99'/>" "$xml_with_vsock"

xml_without_vsock=$(_backend_libvirt_create_vm \
    "agent-02" "$base_disk" "$seed_iso" \
    2 4096 "default" "52:54:00:12:34:57" false "" "" \
    4096 400 500 655000000 262000000 \
    "" "")
assert_not_contains "XML omits vsock block when CID omitted" "<vsock" "$xml_without_vsock"

echo ""
echo "=== Test: cleanup paths remove VSock registry rows ==="

source "$ROOT_DIR/scripts/destroy-vm.sh"

AGENT_DEL_ID="018fb9f1-0009-7000-8000-000000000009"
AGENT_KEEP_ID="018fb9f1-0010-7000-8000-000000000010"
write_vm_info "agent-del" "$AGENT_DEL_ID" "9"
write_vm_info "agent-keep" "$AGENT_KEEP_ID" "10"
printf '9=%s\n10=%s\n' "$AGENT_DEL_ID" "$AGENT_KEEP_ID" > "$CID_REGISTRY"
remove_cid_allocation "agent-del"
if [[ -f "$CID_REGISTRY" ]] && ! grep -q "$AGENT_DEL_ID" "$CID_REGISTRY"; then
    pass "destroy-vm cleanup removes registry row for VM"
else
    fail "destroy-vm cleanup removes registry row for VM"
fi
if grep -q "^10=$AGENT_KEEP_ID$" "$CID_REGISTRY"; then
    pass "destroy-vm cleanup preserves unrelated registry rows"
else
    fail "destroy-vm cleanup preserves unrelated registry rows"
fi

# Reaper VM names must match is_e2e_vm (^agentic-e2e-[0-9]+$). Non-current rows
# are removed; the --current VM is retained (and its dir/vm-info preserved).
reap_vm_root="$TMP_ROOT/e2e-root"
mkdir -p "$reap_vm_root/agentic-e2e-001" "$reap_vm_root/agentic-e2e-002" "$reap_vm_root/agentic-prod"
cat > "$reap_vm_root/agentic-e2e-001/vm-info.json" <<'EOF'
{
  "instance_id": "018fb9f1-2001-7000-8000-000000002001",
  "vsock_cid": "3"
}
EOF
cat > "$reap_vm_root/agentic-e2e-002/vm-info.json" <<'EOF'
{
  "instance_id": "018fb9f1-2002-7000-8000-000000002002",
  "vsock_cid": "4"
}
EOF
printf '3=018fb9f1-2001-7000-8000-000000002001\n4=018fb9f1-2002-7000-8000-000000002002\n9=agentic-prod\n' > "$CID_REGISTRY"
printf 'agentic-e2e-001=201\nagentic-e2e-002=202\nagentic-prod=203\n' > "$IP_REGISTRY"

"$ROOT_DIR/scripts/reap-e2e-vms.sh" \
    --skip-libvirt \
    --vm-root "$reap_vm_root" \
    --cid-registry "$CID_REGISTRY" \
    --ip-registry "$IP_REGISTRY" \
    --current agentic-e2e-002 \
    >/tmp/agentic-reap.log 2>&1

assert_not_contains "reap removes stale E2E CID rows" "agentic-e2e-001" "$CID_REGISTRY"
assert_not_contains "reap removes stale E2E canonical CID rows" "018fb9f1-2001-7000-8000-000000002001" "$CID_REGISTRY"
assert_contains "reap preserves current E2E CID row" "4=018fb9f1-2002-7000-8000-000000002002" "$CID_REGISTRY"
assert_contains "reap preserves non-E2E CID row" "9=agentic-prod" "$CID_REGISTRY"
assert_not_contains "reap removes stale E2E IP rows in parallel" "agentic-e2e-001=" "$IP_REGISTRY"
assert_contains "reap preserves current E2E IP row" "agentic-e2e-002=202" "$IP_REGISTRY"

echo ""
echo "=== Test: cid registry reconciliation against vm-info and malformed entries ==="
# The reaper removes non-current e2e dirs (and their vm-info) before the CID
# sweep, so the reconcile-against-vm-info path is reachable for the retained
# (--current) VM: its registry CID disagrees with vm-info and must be repaired.
reap_vm_root="$TMP_ROOT/e2e-root-reconcile"
mkdir -p \
    "$reap_vm_root/agentic-e2e-001" \
    "$reap_vm_root/agentic-e2e-002"
printf '{\n  "instance_id": "018fb9f1-3001-7000-8000-000000003001",\n  "vsock_cid": "17"\n}\n' > "$reap_vm_root/agentic-e2e-001/vm-info.json"
printf '{\n  "instance_id": "018fb9f1-3002-7000-8000-000000003002",\n  "vsock_cid": "4"\n}\n' > "$reap_vm_root/agentic-e2e-002/vm-info.json"
printf '17=018fb9f1-3001-7000-8000-000000003001\n99=018fb9f1-3002-7000-8000-000000003002\n18=agentic-prod\nbad-row\n' > "$CID_REGISTRY"

"$ROOT_DIR/scripts/reap-e2e-vms.sh" \
    --skip-libvirt \
    --vm-root "$reap_vm_root" \
    --cid-registry "$CID_REGISTRY" \
    --ip-registry "$IP_REGISTRY" \
    --current agentic-e2e-002 \
    >/tmp/agentic-reap-reconcile.log 2>&1

assert_not_contains "reap removes malformed cid registry row" "bad-row" "$CID_REGISTRY"
assert_not_contains "reap removes stale e2e row without vm-info or domain" "018fb9f1-3001-7000-8000-000000003001" "$CID_REGISTRY"
assert_contains "reap reconciles current e2e cid row using vm-info" "4=018fb9f1-3002-7000-8000-000000003002" "$CID_REGISTRY"
assert_not_contains "reconcile drops stale pre-reconcile cid value" "99=018fb9f1-3002-7000-8000-000000003002" "$CID_REGISTRY"
assert_contains "reap preserves non-e2e row" "18=agentic-prod" "$CID_REGISTRY"

echo ""
echo "=== Summary ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
if (( FAIL > 0 )); then
    echo "Failures:"
    for err in "${ERRORS[@]}"; do
        echo " - $err"
    done
    exit 1
fi

echo "vsock CID lifecycle regression tests passed"
