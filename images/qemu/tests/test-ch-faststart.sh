#!/usr/bin/env bash
# Regression coverage for CH fast-start wrapper semantics:
# - pre-enrollment snapshots write provenance and secret posture
# - restore verifies provenance, allocates a fresh CID, and writes enroll-on-restore metadata
# - secret-bearing snapshots require sealing material

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"
QEMU_DIR="$ROOT_DIR/images/qemu"

PASS=0
FAIL=0
ERRORS=()

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); ERRORS+=("$1"); }

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

assert_not_exists() {
    local label="$1"
    local path="$2"
    if [[ ! -e "$path" ]]; then
        pass "$label"
    else
        fail "$label (unexpected path: $path)"
    fi
}

assert_exists() {
    local label="$1"
    local path="$2"
    if [[ -e "$path" ]]; then
        pass "$label"
    else
        fail "$label (missing path: $path)"
    fi
}

assert_mode() {
    local label="$1"
    local expected="$2"
    local path="$3"
    local actual
    actual="$(stat -c '%a' "$path" 2>/dev/null || true)"
    if [[ "$actual" == "$expected" ]]; then
        pass "$label"
    else
        fail "$label (expected mode $expected, got ${actual:-missing})"
    fi
}

TMP_ROOT="$(mktemp -d)"
cleanup() {
    if [[ -n "${API_SOCKET_PID:-}" ]]; then
        kill "$API_SOCKET_PID" 2>/dev/null || true
    fi
    if [[ -d "$TMP_ROOT/vms" ]]; then
        while IFS= read -r pid_file; do
            kill "$(cat "$pid_file")" 2>/dev/null || true
        done < <(find "$TMP_ROOT/vms" -name pid -type f 2>/dev/null)
    fi
    rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

mkdir -p "$TMP_ROOT/fakebin" "$TMP_ROOT/vms/base-vm/cloud-hypervisor" "$TMP_ROOT/agentshare/global-ro"
mkdir -p "$TMP_ROOT/agentshare/base-vm-inbox" "$TMP_ROOT/agentshare/base-vm-outbox"
export CH_LOG="$TMP_ROOT/cloud-hypervisor.log"
export CH_REMOTE_LOG="$TMP_ROOT/ch-remote.log"
export GPG_LOG="$TMP_ROOT/gpg.log"
export CH_SNAPSHOT_SIGNER_FINGERPRINT="0123456789ABCDEF0123456789ABCDEF01234567"
export IP_LOG="$TMP_ROOT/ip.log"
export SUDO_LOG="$TMP_ROOT/sudo.log"
export VM_STORAGE_DIR="$TMP_ROOT/vms"
export CH_SNAPSHOT_ROOT="$TMP_ROOT/vms/.ch-snapshots"
export CH_WARM_POOL_ROOT="$TMP_ROOT/vms/.ch-warm-pool"
export CH_RESTORE_LATENCY_BUDGET_MS=5000
export CH_WARM_HANDOFF_READY_LATENCY_BUDGET_MS=5000
export AGENTSHARE_ROOT="$TMP_ROOT/agentshare"
export AGENTIC_BACKEND="cloud-hypervisor"
export AGENTIC_CH_FIRMWARE="$TMP_ROOT/CLOUDHV.fd"
export AGENTIC_CH_SKIP_DEVICE_CHECKS=1
export AGENTIC_CH_GUEST_SSH_KEY="$TMP_ROOT/fake-guest-key"
touch "$AGENTIC_CH_FIRMWARE" "$AGENTIC_CH_GUEST_SSH_KEY"

if grep -A5 '^ms_now()' "$QEMU_DIR/ch-faststart.sh" | grep -qF '/proc/uptime'; then
    pass "fast-start latency budgets use a monotonic clock"
else
    fail "fast-start latency budgets use a monotonic clock"
fi

cat > "$TMP_ROOT/fakebin/cloud-hypervisor" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$CH_LOG"
api_socket=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--api-socket" ]]; then
    api_socket="$arg"
    break
  fi
  prev="$arg"
done
if [[ -n "$api_socket" ]]; then
  API_SOCKET="$api_socket" python3 - <<'PY' &
import os
import socket
import time

path = os.environ["API_SOCKET"]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX)
s.bind(path)
s.listen(1)
time.sleep(60)
PY
fi
sleep 60
EOF

cat > "$TMP_ROOT/fakebin/ch-remote" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$CH_REMOTE_LOG"
last="${*: -1}"
prev="${*: -2:1}"
if [[ "$prev" == "snapshot" ]]; then
  dir="${last#file://}"
  mkdir -p "$dir"
  printf '{"state":"snapshot"}\n' > "$dir/state.json"
  printf '{"disks":[],"net":[{"id":"base-net","tap":"base-tap","mac":"52:54:00:12:34:56"}],"fs":[]}\n' > "$dir/config.json"
  printf 'memory' > "$dir/memory-ranges"
fi
if [[ "$last" == "info" ]]; then
  printf '{"config":{"net":[{"id":"base-net"}],"fs":[]}}\n'
fi
EOF

cat > "$TMP_ROOT/fakebin/gpg" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$GPG_LOG"
out=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "-o" ]]; then
    out="$arg"
  fi
  prev="$arg"
done
if [[ " $* " == *" --fingerprint "* ]]; then
  printf 'fpr:::::::::0123456789ABCDEF0123456789ABCDEF01234567:\n'
  exit 0
fi
if [[ " $* " == *" --verify "* ]]; then
  printf '[GNUPG:] VALIDSIG 0123456789ABCDEF0123456789ABCDEF01234567 2026-07-21 0 4 0 1 10 00 0123456789ABCDEF0123456789ABCDEF01234567\n'
  exit 0
fi
[[ -n "$out" ]] || exit 2
if [[ " $* " == *" --detach-sign "* ]]; then
  printf 'test-signature\n' > "$out"
elif [[ " $* " == *" --symmetric "* ]]; then
  if [[ "${FAKE_GPG_FAIL_SYMMETRIC:-}" == "1" ]]; then
    exit 9
  fi
  cp "${*: -1}" "$out"
else
  exit 2
fi
EOF

cat > "$TMP_ROOT/fakebin/ssh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
printf '{"boot_id":"00000000-0000-0000-0000-000000000001","ssh_host_key_b64":"c3NoLWVkMjU1MTkgQUFBQUMzTnphQzFsWkRJMU5URTVBQUFBQUlBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBIGJhc2UK","agent_inactive":true,"no_tls_identity":true,"no_bootstrap_or_oauth":true,"credential_mounts_detached":true}\n'
EOF

cat > "$TMP_ROOT/fakebin/ip" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$IP_LOG"
if [[ "${1:-}" == "link" && "${2:-}" == "show" ]]; then
  [[ "${3:-}" == "virbr0" ]] && exit 0
  exit 1
fi
exit 0
EOF

cat > "$TMP_ROOT/fakebin/sudo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$SUDO_LOG"
if [[ "${1:-}" == "-n" ]]; then
  shift
fi
exec "$@"
EOF

cat > "$TMP_ROOT/fakebin/virtiofsd" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
socket=""
for arg in "$@"; do
  case "$arg" in --socket-path=*) socket="${arg#--socket-path=}";; esac
done
if [[ -n "$socket" ]]; then
  SOCKET_PATH="$socket" python3 - <<'PY'
import os
import socket
import time

path = os.environ["SOCKET_PATH"]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX)
s.bind(path)
s.listen(1)
time.sleep(60)
PY
fi
EOF

chmod 0755 "$TMP_ROOT/fakebin/"*
export PATH="$TMP_ROOT/fakebin:$PATH"
export AGENTIC_CH_BIN="$TMP_ROOT/fakebin/cloud-hypervisor"
export AGENTIC_CH_REMOTE_BIN="$TMP_ROOT/fakebin/ch-remote"
export AGENTIC_CH_VIRTIOFSD_BIN="$TMP_ROOT/fakebin/virtiofsd"

base_state_dir="$TMP_ROOT/vms/base-vm/cloud-hypervisor"
base_disk="$TMP_ROOT/vms/base-vm/base-vm.qcow2"
printf 'disk' > "$base_disk"
api_socket="$base_state_dir/api.sock"
API_SOCKET="$api_socket" python3 - <<'PY' &
import os
import socket
import time

path = os.environ["API_SOCKET"]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX)
s.bind(path)
s.listen(1)
time.sleep(60)
PY
API_SOCKET_PID="$!"
for _ in $(seq 1 50); do
    [[ -S "$api_socket" ]] && break
    sleep 0.1
done

cat > "$base_state_dir/vm.env" <<EOF
VM_NAME=base-vm
DISK_PATH=$base_disk
CLOUD_INIT_ISO=$TMP_ROOT/vms/base-vm/cloud-init.iso
CPUS=2
MEMORY_MB=2048
NETWORK=default
BRIDGE=virbr0
MAC_ADDRESS=52:54:00:12:34:56
TAP_NAME=asbase
USE_AGENTSHARE=true
INBOX_PATH=$TMP_ROOT/agentshare/base-vm-inbox
OUTBOX_PATH=$TMP_ROOT/agentshare/base-vm-outbox
CARBONYL_SESSION_PATH=
VSOCK_CID=42
VSOCK_SOCKET=$base_state_dir/vsock.sock
API_SOCKET=$api_socket
PID_FILE=$base_state_dir/pid
SERIAL_LOG=$base_state_dir/serial.log
MEM_LIMIT_MB=2048
CPU_QUOTA_PCT=200
IO_WEIGHT=500
IO_READ_BPS=524288000
IO_WRITE_BPS=209715200
EOF

echo "=== Test: clean-base snapshot and provenance ==="
"$QEMU_DIR/ch-faststart.sh" clean-prepare --vm base-vm --host 192.0.2.10 --user agent \
    > "$TMP_ROOT/clean-prepare.json"
"$QEMU_DIR/ch-faststart.sh" snapshot --vm base-vm --id clean-base --pre-enrollment --sign-key test-signing-key > "$TMP_ROOT/snapshot.json"
snapshot_dir="$CH_SNAPSHOT_ROOT/clean-base"
assert_contains "snapshot output reports pre-enrollment" '"pre_enrollment":true' "$TMP_ROOT/snapshot.json"
assert_contains "secret posture marks clean base" '"posture": "clean-base"' "$snapshot_dir/secret-posture.json"
assert_contains "provenance includes backend metadata" "backend-metadata.json" "$snapshot_dir/provenance.sha256"
assert_exists "clean base has detached provenance signature" "$snapshot_dir/provenance.sha256.sig"
assert_contains "clean base pins signer fingerprint" "$CH_SNAPSHOT_SIGNER_FINGERPRINT" "$snapshot_dir/secret-posture.json"
assert_not_exists "clean-base attestation is single-use" "$base_state_dir/clean-base-attestation.json"
assert_mode "clean base memory artifact is read-only" "444" "$snapshot_dir/ch-state/memory-ranges"

echo ""
echo "=== Test: GPU/VFIO snapshots cannot enter restore, fork, or warm pools ==="
gpu_snapshot_dir="$CH_SNAPSHOT_ROOT/gpu-base"
cp -a "$snapshot_dir" "$gpu_snapshot_dir"
printf 'GPU_ENABLED=true\nGPU_PCI_DEVICE=0000:41:00.0\n' >> "$gpu_snapshot_dir/source-vm.env"
rm -f "$gpu_snapshot_dir/provenance.sha256" "$gpu_snapshot_dir/provenance.sha256.sig"
(
    cd "$gpu_snapshot_dir"
    find . -maxdepth 2 -type f \
        ! -name provenance.sha256 \
        ! -name provenance.sha256.sig \
        ! -name sealed-snapshot.gpg \
        ! -name 'sealed-snapshot.gpg.*' \
        -print0 | sort -z | xargs -0 sha256sum
) > "$gpu_snapshot_dir/provenance.sha256"
gpg --batch --yes --local-user test-signing-key --detach-sign \
    -o "$gpu_snapshot_dir/provenance.sha256.sig" "$gpu_snapshot_dir/provenance.sha256"
if "$QEMU_DIR/ch-faststart.sh" warm-init --snapshot gpu-base --size 1 --prefix gpu-pool \
    >"$TMP_ROOT/gpu-warm.out" 2>"$TMP_ROOT/gpu-warm.err"; then
    fail "GPU snapshot is rejected from warm-pool initialization"
else
    pass "GPU snapshot is rejected from warm-pool initialization"
fi
assert_contains "GPU warm-pool rejection explains cold hand-out policy" \
    "reset-gated cold VM" "$TMP_ROOT/gpu-warm.err"
assert_not_exists "GPU warm-pool rejection creates no pool" "$CH_WARM_POOL_ROOT/gpu-pool"

echo ""
echo "=== Test: restore verifies provenance and fresh identity metadata ==="
printf '%s\n' '{"single":{"instance_id":"child-instance","token":"restore-token","spiffe_id":"spiffe://sandbox.agentic.local/agent/child-instance","expires_at_unix_ms":1784319999000,"tls_dir":"/etc/agentic-sandbox/grpc-mtls","enrollment_url":"https://host.internal:8124/api/v1/bootstrap-enrollment/consume","ca_pem":"-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----"}}' \
    | CH_BOOTSTRAP_STDIN=1 "$QEMU_DIR/ch-faststart.sh" restore --snapshot clean-base --name child-one --mode ondemand --instance-id child-instance > "$TMP_ROOT/restore.json"
assert_contains "restore output reports enroll-on-restore" '"enroll_on_restore":true' "$TMP_ROOT/restore.json"
assert_contains "restore output reports bootstrap token issuance" '"bootstrap_token_issued":true' "$TMP_ROOT/restore.json"
assert_contains "restore wrote fresh CID evidence" '"fresh_vsock_cid": 3' "$TMP_ROOT/vms/child-one/enroll-on-restore.json"
assert_contains "restore records bootstrap SPIFFE without raw token" '"bootstrap_spiffe_id": "spiffe://sandbox.agentic.local/agent/child-instance"' "$TMP_ROOT/vms/child-one/enroll-on-restore.json"
assert_not_exists "restore does not persist a redundant host token sidecar" "$TMP_ROOT/vms/child-one/restore-bootstrap.env"
assert_contains "restore writes guest-visible one-time token" "AGENT_BOOTSTRAP_TOKEN=restore-token" "$TMP_ROOT/agentshare/child-one-inbox/restore-bootstrap.env"
assert_contains "restore overrides the cloned base agent id" "AGENT_ID=child-instance" "$TMP_ROOT/agentshare/child-one-inbox/restore-bootstrap.env"
assert_contains "restore delivers the host bridge address without DNS discovery" "AGENT_RESTORE_HOST_ADDRESS=192.168.122.1" "$TMP_ROOT/agentshare/child-one-inbox/restore-bootstrap.env"
assert_contains "restore metadata records guest bootstrap mount path" '"guest_bootstrap_env_mount_path": "/mnt/inbox/restore-bootstrap.env"' "$TMP_ROOT/vms/child-one/enroll-on-restore.json"
assert_contains "restore metadata records inbox-only bootstrap delivery" '"bootstrap_delivery": "agentshare-inbox"' "$TMP_ROOT/vms/child-one/enroll-on-restore.json"
assert_contains "restore guest sidecar is access scoped" '"guest_bootstrap_env_mode": "600"' "$TMP_ROOT/vms/child-one/enroll-on-restore.json"
assert_contains "restore child gets isolated inbox" "INBOX_PATH=$TMP_ROOT/agentshare/child-one-inbox" "$TMP_ROOT/vms/child-one/cloud-hypervisor/vm.env"
assert_contains "restore child gets isolated outbox" "OUTBOX_PATH=$TMP_ROOT/agentshare/child-one-outbox" "$TMP_ROOT/vms/child-one/cloud-hypervisor/vm.env"
assert_contains "restore uses CH ondemand mode" "memory_restore_mode=ondemand" "$CH_LOG"
assert_contains "CID registry uses instance identity" "3=child-instance" "$TMP_ROOT/vms/.vsock-cid-registry"

echo ""
echo "=== Test: fork and warm pool wrappers ==="
printf '%s\n' '{"children":{"fork-child-1":{"token":"fork-token-1","spiffe_id":"spiffe://sandbox.agentic.local/agent/fork-child-1","expires_at_unix_ms":1784319999001,"tls_dir":"/etc/agentic-sandbox/grpc-mtls","enrollment_url":"https://host.internal:8124/api/v1/bootstrap-enrollment/consume","ca_pem":"-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----"},"fork-child-2":{"token":"fork-token-2","spiffe_id":"spiffe://sandbox.agentic.local/agent/fork-child-2","expires_at_unix_ms":1784319999002,"tls_dir":"/etc/agentic-sandbox/grpc-mtls","enrollment_url":"https://host.internal:8124/api/v1/bootstrap-enrollment/consume","ca_pem":"-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----"}}}' \
    | CH_BOOTSTRAP_STDIN=1 "$QEMU_DIR/ch-faststart.sh" fork --snapshot clean-base --prefix fork-child --count 2 --mode ondemand > "$TMP_ROOT/fork.json"
assert_contains "fork output includes first child" '"name":"fork-child-1"' "$TMP_ROOT/fork.json"
assert_contains "fork manifest records per-child COW" '"disk_cow_per_child": true' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
assert_contains "fork manifest records RAM-sharing summary" '"memory_sharing": {' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
assert_contains "fork manifest records per-child RAM sample count" '"sample_count":2' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
assert_contains "fork manifest records aggregate RSS" '"total_rss_kb":' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
assert_contains "fork manifest records estimated shared savings" '"estimated_shared_savings_kb":' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
assert_contains "fork manifest scopes evidence to guest RAM mappings" '"evidence_scope":"guest RAM mappings only (/memfd:ch_ram); process-wide library sharing excluded"' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
assert_contains "fork manifest does not overclaim unavailable resident sharing" '"claim":"insufficient-evidence"' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
assert_contains "fork manifest records UFFDIO_COPY restore semantics" '"restore_mechanism":"userfaultfd UFFDIO_COPY into each child guest-memory mapping"' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
"$QEMU_DIR/ch-faststart.sh" fork-evidence --prefix fork-child --count 2 > "$TMP_ROOT/fork-evidence.json"
assert_contains "fork evidence resamples the running fan-out" '"memory_evidence_sampled_at":' "$TMP_ROOT/fork-evidence.json"
assert_contains "fork evidence updates the durable manifest" '"memory_evidence_sampled_at":' "$TMP_ROOT/vms/fork-child-fork-manifest.json"
assert_not_exists "fork child one has no redundant host token sidecar" "$TMP_ROOT/vms/fork-child-1/restore-bootstrap.env"
assert_not_exists "fork child two has no redundant host token sidecar" "$TMP_ROOT/vms/fork-child-2/restore-bootstrap.env"
assert_contains "fork child one guest drop is isolated" "AGENT_BOOTSTRAP_TOKEN=fork-token-1" "$TMP_ROOT/agentshare/fork-child-1-inbox/restore-bootstrap.env"
assert_contains "fork child two guest drop is isolated" "AGENT_BOOTSTRAP_TOKEN=fork-token-2" "$TMP_ROOT/agentshare/fork-child-2-inbox/restore-bootstrap.env"
assert_mode "fork child shared memory source stays read-only" "444" "$TMP_ROOT/vms/fork-child-1/cloud-hypervisor/restore-source/memory-ranges"
(
    ready=0
    while (( ready < 2 )); do
        for drop in "$TMP_ROOT"/agentshare/pool-a-slot-*-inbox/restore-bootstrap.env; do
            [[ -f "$drop" ]] || continue
            inbox="$(dirname "$drop")"
            [[ -f "$inbox/warm-slot-ready.json" ]] && continue
            printf '%s\n' '{"schema":1,"network_ready":true,"credential_free":true}' > "$inbox/warm-slot-ready.json"
            chmod 600 "$inbox/warm-slot-ready.json"
            rm -f "$drop"
            ready=$((ready + 1))
        done
        sleep 0.01
    done
) &
WARM_PREPARER_PID=$!
"$QEMU_DIR/ch-faststart.sh" warm-init --snapshot clean-base --size 2 --prefix pool-a > "$TMP_ROOT/warm-init.json"
wait "$WARM_PREPARER_PID"
assert_contains "warm pool records idle count" '"idle": 2' "$TMP_ROOT/vms/.ch-warm-pool/pool-a/pool.json"
assert_contains "warm pool records real paused slots" '"state": "ready_paused"' "$TMP_ROOT/vms/.ch-warm-pool/pool-a/pool.json"
(
    drop="$TMP_ROOT/agentshare/warm-child-inbox/restore-bootstrap.env"
    for _ in $(seq 1 100); do
        if [[ -f "$drop" ]]; then
            rm -f "$drop"
            printf '%s\n' '{"schema":1,"spiffe_id":"spiffe://sandbox.agentic.local/agent/warm-instance","certificate_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","tls_materialized":true}' \
                > "$(dirname "$drop")/restore-enrollment-ready.json"
            chmod 600 "$(dirname "$drop")/restore-enrollment-ready.json"
            exit 0
        fi
        sleep 0.01
    done
    exit 1
) &
WARM_CONSUMER_PID=$!
(
    for _ in $(seq 1 300); do
        for drop in "$TMP_ROOT"/agentshare/pool-a-slot-*-inbox/restore-bootstrap.env; do
            [[ -f "$drop" ]] || continue
            grep -q '^AGENT_WARM_PREPARE=true$' "$drop" || continue
            inbox="$(dirname "$drop")"
            printf '%s\n' '{"schema":1,"network_ready":true,"credential_free":true}' > "$inbox/warm-slot-ready.json"
            chmod 600 "$inbox/warm-slot-ready.json"
            rm -f "$drop"
            exit 0
        done
        sleep 0.01
    done
    exit 1
) &
WARM_REPLENISHER_PID=$!
printf '%s\n' '{"single":{"instance_id":"warm-instance","token":"warm-token","spiffe_id":"spiffe://sandbox.agentic.local/agent/warm-instance","expires_at_unix_ms":1784319999003,"tls_dir":"/etc/agentic-sandbox/grpc-mtls","enrollment_url":"https://host.internal:8124/api/v1/bootstrap-enrollment/consume","ca_pem":"-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----"}}' \
    | CH_BOOTSTRAP_STDIN=1 CH_ENROLLMENT_READY_TIMEOUT_MS=2000 \
      "$QEMU_DIR/ch-faststart.sh" warm-handoff --pool pool-a --name warm-child \
      --instance-id warm-instance --wait-enrollment-ready > "$TMP_ROOT/warm-handoff.json"
wait "$WARM_CONSUMER_PID"
wait "$WARM_REPLENISHER_PID"
for _ in $(seq 1 200); do
    [[ "$(jq -r '.idle' "$TMP_ROOT/vms/.ch-warm-pool/pool-a/pool.json")" == "2" ]] && break
    sleep 0.01
done
assert_contains "warm handoff restores child" '"name":"warm-child"' "$TMP_ROOT/warm-handoff.json"
assert_contains "warm handoff proves enrollment readiness" '"enrollment_ready":true' "$TMP_ROOT/warm-handoff.json"
assert_contains "warm pool marks the claimed slot handed out" '"state": "handed_out"' "$TMP_ROOT/vms/.ch-warm-pool/pool-a/pool.json"
assert_contains "warm pool records completed handout" '"handed_out": 1' "$TMP_ROOT/vms/.ch-warm-pool/pool-a/pool.json"
assert_contains "warm pool asynchronously replenishes capacity" '"idle": 2' "$TMP_ROOT/vms/.ch-warm-pool/pool-a/pool.json"

echo ""
echo "=== Test: secret-bearing snapshot guard ==="
printf 'keep\n' > "$TMP_ROOT/outside-snapshot-sentinel"
if "$QEMU_DIR/ch-faststart.sh" snapshot --vm base-vm --id ../outside-snapshot-sentinel \
    --pre-enrollment --sign-key test-signing-key \
    >"$TMP_ROOT/traversal.out" 2>"$TMP_ROOT/traversal.err"; then
    fail "snapshot id path traversal is rejected"
else
    pass "snapshot id path traversal is rejected"
fi
assert_exists "snapshot traversal rejection preserves outside path" "$TMP_ROOT/outside-snapshot-sentinel"

if "$QEMU_DIR/ch-faststart.sh" snapshot --vm base-vm --id unsigned --pre-enrollment >"$TMP_ROOT/unsigned.out" 2>"$TMP_ROOT/unsigned.err"; then
    fail "unsigned clean-base snapshot is rejected"
else
    pass "unsigned clean-base snapshot is rejected"
fi
assert_contains "clean-base signature guard explains signing requirement" "must be signed" "$TMP_ROOT/unsigned.err"
assert_not_exists "unsigned clean-base snapshot leaves no bundle" "$CH_SNAPSHOT_ROOT/unsigned"

if "$QEMU_DIR/ch-faststart.sh" snapshot --vm base-vm --id unsafe --secret-bearing >"$TMP_ROOT/unsafe.out" 2>"$TMP_ROOT/unsafe.err"; then
    fail "secret-bearing snapshot without seal key is rejected"
else
    pass "secret-bearing snapshot without seal key is rejected"
fi
assert_contains "secret-bearing guard explains seal key" "seal-key" "$TMP_ROOT/unsafe.err"

printf 'test snapshot passphrase\n' > "$TMP_ROOT/seal.key"
chmod 600 "$TMP_ROOT/seal.key"
if FAKE_GPG_FAIL_SYMMETRIC=1 "$QEMU_DIR/ch-faststart.sh" snapshot --vm base-vm \
    --id failed-seal --secret-bearing --seal-key "$TMP_ROOT/seal.key" \
    >"$TMP_ROOT/failed-seal.out" 2>"$TMP_ROOT/failed-seal.err"; then
    fail "failed secret-bearing encryption is rejected"
else
    pass "failed secret-bearing encryption is rejected"
fi
assert_not_exists "failed secret-bearing encryption removes plaintext bundle" "$CH_SNAPSHOT_ROOT/failed-seal"

"$QEMU_DIR/ch-faststart.sh" snapshot --vm base-vm --id sealed-live --secret-bearing \
    --seal-key "$TMP_ROOT/seal.key" --sign-key test-signing-key > "$TMP_ROOT/sealed-live.json"
sealed_dir="$CH_SNAPSHOT_ROOT/sealed-live"
assert_exists "secret-bearing snapshot writes sealed bundle" "$sealed_dir/sealed-snapshot.gpg"
assert_exists "secret-bearing snapshot writes ciphertext fixity" "$sealed_dir/sealed-snapshot.gpg.sha256"
assert_exists "secret-bearing snapshot signs sealed bundle" "$sealed_dir/sealed-snapshot.gpg.sig"
assert_not_exists "secret-bearing snapshot removes plaintext CH memory state" "$sealed_dir/ch-state"
assert_not_exists "secret-bearing snapshot removes plaintext backend metadata" "$sealed_dir/backend-metadata.json"
assert_not_exists "secret-bearing snapshot removes plaintext source metadata" "$sealed_dir/source-vm.env"
assert_not_exists "secret-bearing snapshot removes plaintext posture metadata" "$sealed_dir/secret-posture.json"
assert_mode "secret-bearing snapshot directory is owner-only" "700" "$sealed_dir"
assert_mode "sealed snapshot ciphertext is owner-only" "600" "$sealed_dir/sealed-snapshot.gpg"
assert_contains "sealed bundle includes CH memory ranges" "ch-state/memory-ranges" "$sealed_dir/sealed-snapshot.gpg"

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
echo "ch-faststart checks passed"
