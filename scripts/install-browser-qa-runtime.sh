#!/bin/bash
# Install a checksummed Carbonyl X11 runtime into a disposable browser-QA VM.

set -euo pipefail

usage() {
    echo "Usage: $0 <vm-name> <runtime.tgz> <sha256>" >&2
    exit 2
}

[[ $# -eq 3 ]] || usage
VM_NAME="$1"
ARTIFACT="$2"
EXPECTED_SHA="${3,,}"

[[ -f "$ARTIFACT" ]] || { echo "error: artifact not found: $ARTIFACT" >&2; exit 2; }
[[ "$EXPECTED_SHA" =~ ^[0-9a-f]{64}$ ]] || { echo "error: expected SHA-256 must be 64 lowercase or uppercase hex characters" >&2; exit 2; }

ACTUAL_SHA=$(sha256sum "$ARTIFACT" | awk '{print $1}')
[[ "$ACTUAL_SHA" == "$EXPECTED_SHA" ]] || {
    echo "error: local artifact SHA-256 mismatch: expected $EXPECTED_SHA, got $ACTUAL_SHA" >&2
    exit 1
}

while IFS= read -r member; do
    case "$member" in
        /*|../*|*/../*|*/..)
            echo "error: unsafe archive member: $member" >&2
            exit 1
            ;;
    esac
done < <(tar -tzf "$ARTIFACT")

tar -tzf "$ARTIFACT" | grep -qE '(^|/)carbonyl$' || {
    echo "error: runtime archive does not contain a carbonyl executable" >&2
    exit 1
}

VM_IP=$(virsh -c qemu:///system domifaddr "$VM_NAME" 2>/dev/null \
    | awk '/ipv4/ {print $4}' | cut -d/ -f1 | head -1)
[[ -n "$VM_IP" ]] || { echo "error: could not resolve IP for VM '$VM_NAME'" >&2; exit 2; }

VM_INFO="/var/lib/agentic-sandbox/vms/${VM_NAME}/vm-info.json"
SSH_KEY_PATH=""
if [[ -f "$VM_INFO" ]]; then
    SSH_KEY_PATH=$(python3 - "$VM_INFO" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    print(json.load(handle).get("management", {}).get("ssh_key_path", ""))
PY
)
fi
if [[ -z "$SSH_KEY_PATH" && -e "/var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME}" ]]; then
    SSH_KEY_PATH="/var/lib/agentic-sandbox/secrets/ssh-keys/${VM_NAME}"
fi

SSH_PREFIX=()
SSH_KEY_OPT=()
if [[ -n "$SSH_KEY_PATH" ]]; then
    SSH_KEY_OPT=(-i "$SSH_KEY_PATH")
    if [[ ! -r "$SSH_KEY_PATH" ]]; then
        if sudo -n test -r "$SSH_KEY_PATH" 2>/dev/null; then
            SSH_PREFIX=(sudo)
        else
            echo "error: VM SSH key is not readable: $SSH_KEY_PATH" >&2
            exit 2
        fi
    fi
fi

SSH_OPTS=(-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=10 -o BatchMode=yes "${SSH_KEY_OPT[@]}")
REMOTE_ARTIFACT="/tmp/carbonyl-runtime-${EXPECTED_SHA}.tgz"

"${SSH_PREFIX[@]}" scp "${SSH_OPTS[@]}" "$ARTIFACT" "agent@${VM_IP}:${REMOTE_ARTIFACT}"

"${SSH_PREFIX[@]}" ssh "${SSH_OPTS[@]}" "agent@${VM_IP}" bash -s -- "$REMOTE_ARTIFACT" "$EXPECTED_SHA" <<'REMOTE'
set -euo pipefail
artifact="$1"
expected_sha="$2"
actual_sha=$(sha256sum "$artifact" | awk '{print $1}')
[[ "$actual_sha" == "$expected_sha" ]] || {
    echo "error: remote artifact SHA-256 mismatch: expected $expected_sha, got $actual_sha" >&2
    exit 1
}
sudo install -d -m 0755 /opt/carbonyl
sudo find /opt/carbonyl -mindepth 1 -maxdepth 1 -delete
sudo tar -xzf "$artifact" -C /opt/carbonyl --strip-components=1 --no-same-owner --no-same-permissions
sudo chmod 0755 /opt/carbonyl/carbonyl
rm -f "$artifact"
/opt/carbonyl/carbonyl --version
REMOTE

echo "Installed verified Carbonyl runtime in $VM_NAME ($VM_IP): $EXPECTED_SHA"
