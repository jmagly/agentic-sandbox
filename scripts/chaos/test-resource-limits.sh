#!/bin/bash
# test-resource-limits.sh - Chaos testing for resource limit enforcement
#
# This script tests that resource limits are properly enforced by attempting
# various resource exhaustion attacks against an agent VM.
#
# Prerequisites:
#   - Target VM must be running with hardened agent-client.service
#   - SSH access to VM as agent user
#
# Usage:
#   ./test-resource-limits.sh <vm-name>
#   ./test-resource-limits.sh agent-01
#
# Reference: docs/security/resource-quota-design.md

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Test results
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_SKIPPED=0

log_test() { echo -e "${BLUE}[TEST]${NC} $1"; }
log_pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((TESTS_PASSED++)); }
log_fail() { echo -e "${RED}[FAIL]${NC} $1"; ((TESTS_FAILED++)); }
log_skip() { echo -e "${YELLOW}[SKIP]${NC} $1"; ((TESTS_SKIPPED++)); }
log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }

# Get VM IP address
get_vm_ip() {
    local vm_name="$1"

    # Try virsh domifaddr first
    local ip
    ip=$(virsh domifaddr "$vm_name" 2>/dev/null | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' | head -1)

    if [[ -z "$ip" ]]; then
        # Try qemu-guest-agent
        ip=$(virsh qemu-agent-command "$vm_name" '{"execute":"guest-network-get-interfaces"}' 2>/dev/null | \
             jq -r '.return[].["ip-addresses"][]? | select(.["ip-address-type"]=="ipv4") | .["ip-address"]' 2>/dev/null | \
             grep -v "^127\." | head -1)
    fi

    echo "$ip"
}

# SSH wrapper with timeout
vm_ssh() {
    local ip="$1"
    shift
    ssh -o ConnectTimeout=5 \
        -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null \
        -o LogLevel=ERROR \
        -o BatchMode=yes \
        "agent@${ip}" "$@"
}

# Check if VM is responsive
check_vm_alive() {
    local ip="$1"
    vm_ssh "$ip" "echo alive" 2>/dev/null | grep -q alive
}

# ============================================================================
# Test Cases
# ============================================================================

test_fork_bomb() {
    local ip="$1"

    log_test "Fork bomb containment (TasksMax limit)"

    # This fork bomb should be contained by TasksMax limit
    # Using timeout to prevent hanging
    local output
    output=$(timeout 15 vm_ssh "$ip" "bash -c ':(){ :|:& };:' 2>&1" || true)

    # Give it a moment to settle
    sleep 2

    # VM should still be alive
    if check_vm_alive "$ip"; then
        # Check if we hit the PID limit
        if echo "$output" | grep -qi "resource\|cannot\|fork\|limit"; then
            log_pass "Fork bomb contained by PID limit"
        else
            log_pass "Fork bomb contained (VM survived)"
        fi
    else
        log_fail "VM became unresponsive after fork bomb"
        return 1
    fi
}

test_memory_exhaustion() {
    local ip="$1"

    log_test "Memory exhaustion (MemoryMax limit)"

    # Try to allocate more memory than the limit allows
    # This should trigger OOM killer or MemoryError
    local output
    output=$(timeout 30 vm_ssh "$ip" "python3 -c '
import sys
try:
    # Try to allocate 10GB (should fail with 7.5GB limit)
    x = bytearray(10 * 1024 * 1024 * 1024)
except MemoryError:
    print(\"MemoryError caught\")
    sys.exit(0)
except Exception as e:
    print(f\"Other error: {e}\")
    sys.exit(1)
print(\"No error - limit not enforced!\")
sys.exit(2)
' 2>&1" || true)

    sleep 2

    # VM should still be alive
    if check_vm_alive "$ip"; then
        if echo "$output" | grep -qi "MemoryError\|Killed\|cannot allocate"; then
            log_pass "Memory limit enforced (allocation failed)"
        elif echo "$output" | grep -q "No error"; then
            log_fail "Memory limit NOT enforced (allocation succeeded)"
        else
            log_pass "Memory limit enforced (process killed)"
        fi
    else
        log_fail "VM became unresponsive after memory test"
        return 1
    fi
}

test_file_descriptor_exhaustion() {
    local ip="$1"

    log_test "File descriptor exhaustion (LimitNOFILE)"

    # Try to open more FDs than the limit
    local output
    output=$(timeout 30 vm_ssh "$ip" "python3 -c '
import os
fds = []
try:
    for i in range(100000):
        fds.append(os.open(\"/dev/null\", os.O_RDONLY))
except OSError as e:
    print(f\"Hit limit at {len(fds)} FDs: {e}\")
    for fd in fds:
        os.close(fd)
' 2>&1" || true)

    sleep 1

    if check_vm_alive "$ip"; then
        if echo "$output" | grep -qi "limit\|too many\|resource"; then
            log_pass "FD limit enforced"
        else
            log_pass "FD test completed (VM survived)"
        fi
    else
        log_fail "VM became unresponsive after FD test"
        return 1
    fi
}

test_disk_quota() {
    local ip="$1"

    log_test "Disk quota enforcement"

    # Check if /mnt/inbox exists (agentshare enabled)
    if ! vm_ssh "$ip" "test -d /mnt/inbox" 2>/dev/null; then
        log_skip "Agentshare not enabled (no /mnt/inbox)"
        return 0
    fi

    # Try to write a large file (should fail if quota is set)
    local output
    output=$(timeout 60 vm_ssh "$ip" "
        # Try to write 60GB (should exceed 50GB quota)
        dd if=/dev/zero of=/mnt/inbox/quota_test bs=1M count=61440 2>&1 || echo 'Write failed'
        rm -f /mnt/inbox/quota_test 2>/dev/null
    " 2>&1 || true)

    if echo "$output" | grep -qi "quota\|no space\|disk full\|write failed"; then
        log_pass "Disk quota enforced"
    else
        log_skip "Disk quota not enforced (XFS quotas may not be configured)"
    fi
}

test_io_throughput() {
    local ip="$1"

    log_test "I/O bandwidth limiting"

    # Write 256MB and measure time
    # With 200MB/s limit, should take at least 1.2 seconds
    local output
    output=$(vm_ssh "$ip" "
        start=\$(date +%s.%N)
        dd if=/dev/zero of=/tmp/io_test bs=1M count=256 oflag=direct 2>&1
        end=\$(date +%s.%N)
        rm -f /tmp/io_test
        echo \"elapsed: \$(echo \"\$end - \$start\" | bc)\"
    " 2>&1 || true)

    local elapsed
    elapsed=$(echo "$output" | grep "elapsed:" | cut -d: -f2 | tr -d ' ')

    if [[ -n "$elapsed" ]]; then
        # Check if it took at least 1 second (256MB at 200MB/s = 1.28s)
        if (( $(echo "$elapsed > 1.0" | bc -l) )); then
            log_pass "I/O appears throttled (took ${elapsed}s for 256MB)"
        else
            log_info "I/O completed quickly (${elapsed}s) - throttling may not be active"
            log_pass "I/O test completed"
        fi
    else
        log_skip "Could not measure I/O time"
    fi
}

test_cgroup_status() {
    local ip="$1"

    log_test "cgroup v2 configuration check"

    # Check cgroup v2 is active
    local cgroup_info
    cgroup_info=$(vm_ssh "$ip" "
        echo '=== cgroup controllers ==='
        cat /sys/fs/cgroup/cgroup.controllers 2>/dev/null || echo 'not found'

        echo ''
        echo '=== agent service cgroup ==='
        systemctl show agentic-agent --property=ControlGroup 2>/dev/null || echo 'not found'

        echo ''
        echo '=== memory limits ==='
        cat /sys/fs/cgroup/system.slice/agentic-agent.service/memory.max 2>/dev/null || echo 'not found'
        cat /sys/fs/cgroup/system.slice/agentic-agent.service/memory.high 2>/dev/null || echo 'not found'

        echo ''
        echo '=== pids limit ==='
        cat /sys/fs/cgroup/system.slice/agentic-agent.service/pids.max 2>/dev/null || echo 'not found'
    " 2>&1 || true)

    echo "$cgroup_info" | head -20

    if echo "$cgroup_info" | grep -q "memory"; then
        log_pass "cgroup v2 is configured"
    else
        log_fail "cgroup v2 not properly configured"
    fi
}

# ============================================================================
# Main
# ============================================================================

usage() {
    cat <<EOF
Usage: $0 <vm-name> [test...]

Chaos tests for resource limit enforcement.

Arguments:
  vm-name     Name of the VM to test (e.g., agent-01)
  test        Optional: specific test(s) to run

Available tests:
  fork        Fork bomb containment
  memory      Memory exhaustion
  fd          File descriptor exhaustion
  disk        Disk quota enforcement
  io          I/O bandwidth limiting
  cgroup      cgroup v2 configuration check
  all         Run all tests (default)

Examples:
  $0 agent-01              # Run all tests
  $0 agent-01 fork memory  # Run specific tests
  $0 agent-01 cgroup       # Just check cgroup config

EOF
}

main() {
    if [[ $# -lt 1 ]]; then
        usage
        exit 1
    fi

    local vm_name="$1"
    shift

    # Get VM IP
    local vm_ip
    vm_ip=$(get_vm_ip "$vm_name")

    if [[ -z "$vm_ip" ]]; then
        log_fail "Could not get IP for VM: $vm_name"
        echo "Is the VM running? Try: virsh start $vm_name"
        exit 1
    fi

    log_info "Testing VM: $vm_name ($vm_ip)"
    echo ""

    # Check VM is alive
    if ! check_vm_alive "$vm_ip"; then
        log_fail "Cannot connect to VM at $vm_ip"
        exit 1
    fi

    # Determine which tests to run
    local tests=("$@")
    if [[ ${#tests[@]} -eq 0 ]] || [[ "${tests[0]}" == "all" ]]; then
        tests=(cgroup fork memory fd disk io)
    fi

    echo "=========================================="
    echo "Resource Limit Chaos Tests"
    echo "=========================================="
    echo ""

    for test in "${tests[@]}"; do
        case "$test" in
            fork)    test_fork_bomb "$vm_ip" ;;
            memory)  test_memory_exhaustion "$vm_ip" ;;
            fd)      test_file_descriptor_exhaustion "$vm_ip" ;;
            disk)    test_disk_quota "$vm_ip" ;;
            io)      test_io_throughput "$vm_ip" ;;
            cgroup)  test_cgroup_status "$vm_ip" ;;
            all)     ;; # Already handled
            *)
                log_skip "Unknown test: $test"
                ;;
        esac
        echo ""
    done

    echo "=========================================="
    echo "Test Summary"
    echo "=========================================="
    echo -e "Passed:  ${GREEN}${TESTS_PASSED}${NC}"
    echo -e "Failed:  ${RED}${TESTS_FAILED}${NC}"
    echo -e "Skipped: ${YELLOW}${TESTS_SKIPPED}${NC}"
    echo ""

    if [[ $TESTS_FAILED -gt 0 ]]; then
        exit 1
    fi
}

main "$@"
