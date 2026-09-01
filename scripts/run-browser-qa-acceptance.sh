#!/usr/bin/env bash
# Run Carbonyl full-UI/profile acceptance inside one disposable Ubuntu VM.

set -euo pipefail

usage() {
    echo "Usage: $0 <vm-name> <runtime.tgz> <sha256> <carbonyl-src> <agent-src> <qa-src> <report-dir>" >&2
    exit 2
}

[[ $# -eq 7 ]] || usage
VM_NAME="$1"
RUNTIME_ARTIFACT="$2"
RUNTIME_SHA256="$3"
CARBONYL_SOURCE="$4"
AGENT_SOURCE="$5"
QA_SOURCE="$6"
REPORT_DIR="$7"

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[[ "$VM_NAME" =~ ^carbonyl-qa-[a-z0-9-]+$ ]] || {
    echo "error: VM name must match carbonyl-qa-[a-z0-9-]+" >&2
    exit 2
}
[[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" && -z "${WAYLAND_SOCKET:-}" ]] || {
    echo "error: acceptance orchestrator inherited a host display" >&2
    exit 2
}
for source_dir in "$CARBONYL_SOURCE" "$AGENT_SOURCE" "$QA_SOURCE"; do
    [[ -d "$source_dir/.git" ]] || {
        echo "error: expected a Git checkout: $source_dir" >&2
        exit 2
    }
done

mkdir -p "$REPORT_DIR"
REPORT_DIR="$(cd "$REPORT_DIR" && pwd)"
SCRATCH_DIR="$(mktemp -d "${TMPDIR:-/tmp}/carbonyl-qa-acceptance.XXXXXX")"
VM_CREATED=0

# Keep the disposable VM's SSH, health, and transport material scoped to this
# acceptance run. Shared host secret stores may be root-only, while the
# provision/deploy helpers intentionally run as the invoking operator.
export SECRETS_DIR="$SCRATCH_DIR/secrets"
install -d -m 0700 "$SECRETS_DIR"

# New VMs require a transport-authenticated agent path. Browser QA does not
# consume the management channel, but provisioning still fails closed unless
# cloud-init receives secure transport material. Use the same host-local vsock
# endpoint as the Agentic Sandbox VM benchmark; no host network listener or
# legacy bearer secret is introduced.
export AGENTIC_GRPC_VSOCK_PORT="${AGENTIC_GRPC_VSOCK_PORT:-8120}"

cleanup() {
    status=$?
    trap - EXIT INT TERM
    if (( VM_CREATED )); then
        "$PROJECT_ROOT/scripts/destroy-vm.sh" "$VM_NAME" --force || true
    fi
    rm -rf -- "$SCRATCH_DIR" 2>/dev/null \
        || sudo -n rm -rf -- "$SCRATCH_DIR"
    exit "$status"
}
trap cleanup EXIT INT TERM

git -C "$CARBONYL_SOURCE" archive --format=tar.gz --output="$SCRATCH_DIR/carbonyl-src.tgz" HEAD
git -C "$AGENT_SOURCE" archive --format=tar.gz --output="$SCRATCH_DIR/agent-src.tgz" HEAD
git -C "$QA_SOURCE" archive --format=tar.gz --output="$SCRATCH_DIR/qa-src.tgz" HEAD

VM_CREATED=1
"$PROJECT_ROOT/images/qemu/provision-vm.sh" \
    --base ubuntu-26.04 \
    --loadout profiles/browser-qa.yaml \
    --wait-ready \
    "$VM_NAME"

"$PROJECT_ROOT/scripts/install-browser-qa-runtime.sh" \
    "$VM_NAME" "$RUNTIME_ARTIFACT" "$RUNTIME_SHA256"
"$PROJECT_ROOT/scripts/validate-browser-qa.sh" "$VM_NAME"

VM_INFO="/var/lib/agentic-sandbox/vms/${VM_NAME}/vm-info.json"
VM_IP=$(python3 - "$VM_INFO" <<'PY' 2>/dev/null || true
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    print(json.load(handle).get("ip", ""))
PY
)
if [[ -z "$VM_IP" ]]; then
    VM_IP=$(virsh -c qemu:///system domifaddr "$VM_NAME" 2>/dev/null \
        | awk '/ipv4/ {print $4}' | cut -d/ -f1 | head -1)
fi
[[ -n "$VM_IP" ]] || { echo "error: could not resolve VM IP" >&2; exit 2; }

SSH_KEY_PATH=$(python3 - "$VM_INFO" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    print(json.load(handle).get("management", {}).get("ssh_key_path", ""))
PY
)
[[ -n "$SSH_KEY_PATH" ]] || {
    SSH_KEY_PATH="/var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME}"
}
SSH_PREFIX=()
if [[ ! -r "$SSH_KEY_PATH" ]]; then
    sudo -n test -r "$SSH_KEY_PATH"
    SSH_PREFIX=(sudo)
fi
SSH_OPTS=(
    -i "$SSH_KEY_PATH"
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o LogLevel=ERROR
    -o ConnectTimeout=10
    -o BatchMode=yes
)

"${SSH_PREFIX[@]}" scp "${SSH_OPTS[@]}" \
    "$SCRATCH_DIR/carbonyl-src.tgz" \
    "$SCRATCH_DIR/agent-src.tgz" \
    "$SCRATCH_DIR/qa-src.tgz" \
    "agent@${VM_IP}:/tmp/"

"${SSH_PREFIX[@]}" ssh "${SSH_OPTS[@]}" "agent@${VM_IP}" bash -s <<'REMOTE'
set -euo pipefail

test "$(. /etc/os-release; printf '%s' "$VERSION_ID")" = "26.04"
case "$(systemd-detect-virt --vm)" in
    kvm|qemu) ;;
    *) echo "error: disposable KVM/QEMU guest was not proven" >&2; exit 1 ;;
esac
test -S /tmp/.X11-unix/X99
test -z "${WAYLAND_DISPLAY:-}"
test -z "${WAYLAND_SOCKET:-}"

acceptance_root=/tmp/carbonyl-qa-acceptance
reports=/tmp/carbonyl-qa-reports
rm -rf -- "$acceptance_root" "$reports"
mkdir -p "$acceptance_root/carbonyl" "$acceptance_root/agent" "$acceptance_root/qa" "$reports"
tar -xzf /tmp/carbonyl-src.tgz -C "$acceptance_root/carbonyl"
tar -xzf /tmp/agent-src.tgz -C "$acceptance_root/agent"
tar -xzf /tmp/qa-src.tgz -C "$acceptance_root/qa"

python3 -m venv --system-site-packages "$acceptance_root/venv"
"$acceptance_root/venv/bin/pip" install --quiet --upgrade pip
"$acceptance_root/venv/bin/pip" install --quiet \
    -e "$acceptance_root/agent[dev]" pytest-asyncio docker Pillow pyyaml

export CARBONYL_BIN=/opt/carbonyl/carbonyl
export LD_LIBRARY_PATH=/opt/carbonyl
export CARBONYL_QA_DISPOSABLE_WORKER=1
export CARBONYL_QA_PRIVATE_X11=1
export DISPLAY=:99
export CARBONYL_TEST_NO_SANDBOX=1

KEEP_WORK_DIR=1 bash "$acceptance_root/carbonyl/scripts/test-operator-window.sh" \
    | tee "$reports/operator-window.log"
operator_dir=$(find /tmp -maxdepth 1 -type d -name 'carbonyl-operator-test.*' \
    -printf '%T@ %p\n' | sort -nr | head -1 | cut -d' ' -f2-)
[[ -n "$operator_dir" ]]
cp "$operator_dir"/*.png "$reports/"

env -u DISPLAY -u CARBONYL_QA_PRIVATE_X11 \
    bash "$acceptance_root/carbonyl/scripts/test-storage-flush.sh" \
    | tee "$reports/storage-flush.log"

cd "$acceptance_root/qa"
"$acceptance_root/venv/bin/python" -m pytest \
    tests/integration/test_browser_profile_transition.py \
    --junitxml="$reports/browser-profile-transition.xml" -q

for cycle in 1 2 3; do
    "$acceptance_root/venv/bin/python" -m pytest --runxfail \
        tests/integration/test_layer1_trusted_input.py \
        -m live \
        --junitxml="$reports/layer1-cycle-${cycle}.xml" -q
done

"$acceptance_root/venv/bin/python" -m pytest \
    tests/integration/test_layer6_profile_persistence.py \
    --junitxml="$reports/layer6-profile-persistence.xml" -q

{
    echo "ubuntu=$( . /etc/os-release; printf '%s' "$PRETTY_NAME" )"
    echo "virtualization=$(systemd-detect-virt --vm)"
    echo "runtime_sha256=$(sha256sum /opt/carbonyl/carbonyl | awk '{print $1}')"
    echo "xorg=$(systemctl is-active xorg99.service)"
} > "$reports/environment.txt"
REMOTE

"${SSH_PREFIX[@]}" scp "${SSH_OPTS[@]}" -r \
    "agent@${VM_IP}:/tmp/carbonyl-qa-reports/." "$REPORT_DIR/"

echo "PASS: Ubuntu 26.04 disposable-VM browser acceptance completed"
echo "Evidence: $REPORT_DIR"
