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

echo ""
echo "=== Test: allocate/reuse/reclaim VSock CID registry entries ==="

: > "$CID_REGISTRY"
CID_START=3
CID_END=4
export CID_START CID_END

allocated=$(allocate_cid_for_vm "agent-01")
assert_eq "agent-01 receives first CID in range" "3" "$allocated"

allocated=$(allocate_cid_for_vm "agent-02")
assert_eq "agent-02 receives next CID in range" "4" "$allocated"

allocated=$(allocate_cid_for_vm "agent-02")
assert_eq "agent-02 preserves existing CID" "4" "$allocated"

set +e
allocate_cid_for_vm "agent-03" >/tmp/agentic-cid-fail.log 2>&1
allocate_status=$?
set -e
if [[ "$allocate_status" == "0" ]]; then
    fail "agent-03 fails cleanly when CID range exhausted"
else
    pass "agent-03 fails cleanly when CID range exhausted"
fi

remove_cid_allocation "agent-02"
allocated=$(allocate_cid_for_vm "agent-03")
assert_eq "agent-03 reclaims freed CID" "4" "$allocated"

: > "$CID_REGISTRY"
echo "malformed-line" >> "$CID_REGISTRY"
printf 'agent-01=5\n' >> "$CID_REGISTRY"
CID_START=5
CID_END=6
export CID_START CID_END
allocated=$(allocate_cid_for_vm "custom-name")
assert_eq "custom name gets first-fit CID despite malformed lines" "6" "$allocated"

assert_eq "get_vm_allocated_cid returns deterministic row" "5" "$(get_vm_allocated_cid 'agent-01')"
assert_eq "get_vm_allocated_cid supports custom-name rows" "6" "$(get_vm_allocated_cid 'custom-name')"

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
    ( allocate_cid_for_vm "conc-vm-$i" > "$conc_out/$i" 2>/dev/null ) &
    conc_pids+=("$!")
done
for p in "${conc_pids[@]}"; do wait "$p"; done
mapfile -t conc_cids < <(cat "$conc_out"/* 2>/dev/null | grep -E '^[0-9]+$')
conc_unique=$(printf '%s\n' "${conc_cids[@]}" | sort -nu | grep -c . || true)
assert_eq "16 concurrent allocations returned 16 unique CIDs" "16" "$conc_unique"
assert_eq "16 concurrent allocations recorded 16 registry rows" "16" "$(grep -cE '^conc-vm-[0-9]+=[0-9]+$' "$conc_reg")"
assert_eq "registry holds 16 distinct CID values" "16" "$(awk -F= '{print $2}' "$conc_reg" | sort -nu | grep -c .)"
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

printf 'agent-del=9\nagent-keep=10\n' > "$CID_REGISTRY"
remove_cid_allocation "agent-del"
if [[ -f "$CID_REGISTRY" ]] && ! grep -q '^agent-del=' "$CID_REGISTRY"; then
    pass "destroy-vm cleanup removes registry row for VM"
else
    fail "destroy-vm cleanup removes registry row for VM"
fi
if grep -q '^agent-keep=' "$CID_REGISTRY"; then
    pass "destroy-vm cleanup preserves unrelated registry rows"
else
    fail "destroy-vm cleanup preserves unrelated registry rows"
fi

# Reaper VM names must match is_e2e_vm (^agentic-e2e-[0-9]+$). Non-current rows
# are removed; the --current VM is retained (and its dir/vm-info preserved).
reap_vm_root="$TMP_ROOT/e2e-root"
mkdir -p "$reap_vm_root/agentic-e2e-001" "$reap_vm_root/agentic-e2e-002" "$reap_vm_root/agentic-prod"
printf 'agentic-e2e-001=3\nagentic-e2e-002=4\nagentic-prod=9\n' > "$CID_REGISTRY"
printf 'agentic-e2e-001=201\nagentic-e2e-002=202\nagentic-prod=203\n' > "$IP_REGISTRY"

"$ROOT_DIR/scripts/reap-e2e-vms.sh" \
    --skip-libvirt \
    --vm-root "$reap_vm_root" \
    --cid-registry "$CID_REGISTRY" \
    --ip-registry "$IP_REGISTRY" \
    --current agentic-e2e-002 \
    >/tmp/agentic-reap.log 2>&1

assert_not_contains "reap removes stale E2E CID rows" "agentic-e2e-001" "$CID_REGISTRY"
assert_contains "reap preserves current E2E CID row" "agentic-e2e-002=4" "$CID_REGISTRY"
assert_contains "reap preserves non-E2E CID row" "agentic-prod=9" "$CID_REGISTRY"
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
printf '{\n  "vsock_cid": "4"\n}\n' > "$reap_vm_root/agentic-e2e-002/vm-info.json"
printf 'agentic-e2e-001=17\nagentic-e2e-002=99\nagentic-prod=18\nbad-row\n' > "$CID_REGISTRY"

"$ROOT_DIR/scripts/reap-e2e-vms.sh" \
    --skip-libvirt \
    --vm-root "$reap_vm_root" \
    --cid-registry "$CID_REGISTRY" \
    --ip-registry "$IP_REGISTRY" \
    --current agentic-e2e-002 \
    >/tmp/agentic-reap-reconcile.log 2>&1

assert_not_contains "reap removes malformed cid registry row" "bad-row" "$CID_REGISTRY"
assert_not_contains "reap removes stale e2e row without vm-info or domain" "agentic-e2e-001=17" "$CID_REGISTRY"
assert_contains "reap reconciles current e2e cid row using vm-info" "agentic-e2e-002=4" "$CID_REGISTRY"
assert_not_contains "reconcile drops stale pre-reconcile cid value" "agentic-e2e-002=99" "$CID_REGISTRY"
assert_contains "reap preserves non-e2e row" "agentic-prod=18" "$CID_REGISTRY"

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
