#!/usr/bin/env bash
# Regression guard for #573: the in-guest agent-client install path must be
# identical across the base-image bake, the baked systemd unit, the live-deploy
# script, and the provisioning readiness check. Divergence here is the #561
# failure class (image bakes the agent at one path while provisioning looks at
# another), so this test pins the canonical location.
#
# Canonical path: /opt/agentic-sandbox/bin/agent-client
# (the container tier intentionally uses /usr/local/bin/agent-client and is out
#  of scope for this VM-tier parity check.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"

CANONICAL="/opt/agentic-sandbox/bin/agent-client"
DIVERGENT="/usr/local/bin/agent-client"

PASS=0
FAIL=0
ERRORS=()
pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); ERRORS+=("$1"); }

# assert_file_contains <label> <file> <needle>
assert_file_contains() {
    local label="$1" file="$2" needle="$3"
    if [[ -f "$ROOT_DIR/$file" ]] && grep -qF -- "$needle" "$ROOT_DIR/$file"; then
        pass "$label"
    else
        fail "$label ($file missing or lacks: $needle)"
    fi
}

# assert_file_absent <label> <file> <needle>
assert_file_absent() {
    local label="$1" file="$2" needle="$3"
    if [[ -f "$ROOT_DIR/$file" ]] && grep -qF -- "$needle" "$ROOT_DIR/$file"; then
        fail "$label ($file still references: $needle)"
    else
        pass "$label"
    fi
}

echo "=== Test: canonical agent-client path used across VM tier (#573) ==="

# Base image bakes the binary + enables the unit at the canonical path.
assert_file_contains "build-base-image bakes canonical /opt path" \
    "images/qemu/build-base-image.sh" "$CANONICAL"
# Baked systemd unit launches from the canonical path.
assert_file_contains "agent-client.service ExecStart is canonical /opt path" \
    "agent-rs/systemd/agent-client.service" "ExecStart=$CANONICAL"
assert_file_contains "restore-bootstrap path watches the guest inbox" \
    "agent-rs/systemd/agent-client-restore-bootstrap.path" \
    "PathExists=/mnt/inbox/restore-bootstrap.env"
assert_file_contains "restore-bootstrap path starts the prerequisite trigger" \
    "agent-rs/systemd/agent-client-restore-bootstrap.path" \
    "Unit=agent-client-restore-bootstrap.service"
assert_file_contains "restore-bootstrap timer polls subsecond" \
    "agent-rs/systemd/agent-client-restore-bootstrap.timer" \
    "OnUnitActiveSec=50ms"
assert_file_contains "restore-bootstrap trigger starts the canonical service" \
    "agent-rs/systemd/agent-client-restore-bootstrap-trigger" \
    "agent-client.service"
assert_file_contains "restore-bootstrap trigger mounts the isolated inbox" \
    "agent-rs/systemd/agent-client-restore-bootstrap-trigger" \
    "mount_share agentinbox /mnt/inbox"
assert_file_contains "restore-bootstrap trigger configures a fresh DHCP identity" \
    "agent-rs/systemd/agent-client-restore-bootstrap-trigger" \
    "05-agentic-restore.network"
assert_file_contains "restore-bootstrap network wait is bounded" \
    "agent-rs/systemd/agent-client-restore-bootstrap-trigger" \
    'for _ in {1..20}'
# shellcheck disable=SC2016 -- asserting literal shell source, not expanding it here.
assert_file_contains "restore-bootstrap does not restart an in-flight DHCP lease" \
    "agent-rs/systemd/agent-client-restore-bootstrap-trigger" \
    'if [[ ! -f "$profile" ]]'
assert_file_contains "restore-bootstrap maps the TLS hostname to the VM gateway" \
    "agent-rs/systemd/agent-client-restore-bootstrap-trigger" \
    "AGENT_RESTORE_HOST_ADDRESS"
assert_file_contains "restore-bootstrap DHCP identity follows the fresh MAC" \
    "agent-rs/systemd/agent-client-restore-bootstrap-trigger" \
    "ClientIdentifier=mac"
assert_file_contains "restore-bootstrap waits for the enrollment endpoint" \
    "agent-rs/systemd/agent-client-restore-bootstrap-trigger" \
    "wait_for_restore_enrollment_endpoint"
assert_file_contains "restore-bootstrap prepares the agent-owned TLS directory" \
    "agent-rs/systemd/agent-client-restore-bootstrap-trigger" \
    'install -d -m 0700 -o agent -g agent'
assert_file_contains "restore-bootstrap udev rule activates on virtiofs hot-add" \
    "agent-rs/systemd/99-agentic-restore-bootstrap.rules" \
    'ATTR{device}=="0x001a"'
# Provisioning readiness check polls the canonical path (via constant).
assert_file_contains "provision-vm defines canonical AGENT_CLIENT_BIN" \
    "images/qemu/provision-vm.sh" "AGENT_CLIENT_BIN:-$CANONICAL"
# Live-deploy installs to the canonical path and the unit it writes uses it.
assert_file_contains "provision-vm-agent defines canonical AGENT_CLIENT_BIN" \
    "scripts/provision-vm-agent.sh" "AGENT_CLIENT_BIN:-$CANONICAL"
assert_file_contains "provision-vm-agent unit ExecStart is canonical /opt path" \
    "scripts/provision-vm-agent.sh" "ExecStart=$CANONICAL"

echo ""
echo "=== Test: no divergent /usr/local/bin/agent-client in VM provisioning paths ==="
assert_file_absent "provision-vm.sh has no divergent agent-client path" \
    "images/qemu/provision-vm.sh" "$DIVERGENT"
assert_file_absent "provision-vm-agent.sh has no divergent agent-client path" \
    "scripts/provision-vm-agent.sh" "$DIVERGENT"
assert_file_absent "build-base-image.sh has no divergent agent-client path" \
    "images/qemu/build-base-image.sh" "$DIVERGENT"

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
echo "agent-client path parity checks passed"
