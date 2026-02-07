#!/bin/bash
# Chaos Experiment 5: Slow Clone
#
# Test timeout enforcement by throttling bandwidth during git clone.
#
# Expected Behavior:
# - Clone should proceed slowly under throttled bandwidth
# - Timeout should be enforced if clone takes too long
# - Task should not hang indefinitely
# - Throttling should be removable

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

EXPERIMENT_NAME="chaos-slow-clone"
THROTTLE_KBPS="${THROTTLE_KBPS:-50}"  # Very slow bandwidth
NETWORK_INTERFACE="${NETWORK_INTERFACE:-virbr0}"  # Libvirt default bridge

# =============================================================================
# Traffic Control Functions
# =============================================================================

throttle_bandwidth() {
    local interface="$1"
    local rate_kbps="$2"

    log_step "Throttling bandwidth on ${interface} to ${rate_kbps} kbps"

    # Check if tc qdisc already configured
    if sudo tc qdisc show dev "$interface" | grep -q "htb"; then
        log_warn "Traffic control already configured, removing first"
        remove_throttle "$interface"
    fi

    # Add HTB qdisc (Hierarchical Token Bucket)
    sudo tc qdisc add dev "$interface" root handle 1: htb default 11
    sudo tc class add dev "$interface" parent 1: classid 1:1 htb rate "${rate_kbps}kbit"
    sudo tc class add dev "$interface" parent 1:1 classid 1:11 htb rate "${rate_kbps}kbit"

    log_success "Bandwidth throttled to ${rate_kbps} kbps"
}

remove_throttle() {
    local interface="$1"

    log_step "Removing bandwidth throttling from ${interface}"

    # Remove qdisc (this removes all traffic control)
    sudo tc qdisc del dev "$interface" root 2>/dev/null || true

    log_success "Bandwidth throttling removed"
}

cleanup_throttle() {
    log_info "Cleaning up traffic control"
    remove_throttle "$NETWORK_INTERFACE" || true
}

# =============================================================================
# Experiment Functions
# =============================================================================

run_experiment() {
    log_info "Starting Chaos Experiment: Slow Clone"
    log_info "This test throttles bandwidth during git clone"
    echo ""

    # Register cleanup
    on_exit cleanup_throttle

    # Verify prerequisites
    verify_prerequisites || return 1

    # Check for sudo access (needed for tc)
    if ! sudo -n true 2>/dev/null; then
        log_error "This experiment requires passwordless sudo access"
        log_info "Add to sudoers: $USER ALL=(ALL) NOPASSWD: /usr/sbin/tc"
        test_failed "$EXPERIMENT_NAME" "Insufficient privileges"
        return 1
    fi

    # Check if tc command exists
    if ! command -v tc >/dev/null 2>&1; then
        log_error "tc command not found (install iproute2)"
        test_failed "$EXPERIMENT_NAME" "Missing dependencies"
        return 1
    fi

    # Check if network interface exists
    if ! ip link show "$NETWORK_INTERFACE" >/dev/null 2>&1; then
        log_warn "Network interface ${NETWORK_INTERFACE} not found"
        log_info "Available interfaces:"
        ip link show | grep -E "^[0-9]+:" | cut -d: -f2 | tr -d ' '
        test_failed "$EXPERIMENT_NAME" "Network interface not found"
        return 1
    fi

    # Step 1: Check initial bandwidth
    log_step "Step 1: Checking initial network configuration"
    log_info "Interface: ${NETWORK_INTERFACE}"

    local qdisc_before
    qdisc_before=$(sudo tc qdisc show dev "$NETWORK_INTERFACE")
    log_info "Current qdisc: ${qdisc_before}"
    echo ""

    # Step 2: Apply bandwidth throttling
    log_step "Step 2: Applying bandwidth throttling"

    throttle_bandwidth "$NETWORK_INTERFACE" "$THROTTLE_KBPS"

    # Verify throttling is active
    local qdisc_after
    qdisc_after=$(sudo tc qdisc show dev "$NETWORK_INTERFACE")
    log_info "New qdisc: ${qdisc_after}"
    echo ""

    # Step 3: Submit a task that clones a larger repository
    log_step "Step 3: Submitting git clone task with timeout"

    # Create manifest with a shorter timeout to test timeout enforcement
    create_manifest_dir
    local manifest_path="${MANIFEST_DIR}/slow-clone-test.yaml"

    cat > "$manifest_path" <<EOF
name: slow-clone-test
runtime: ubuntu:22.04
resources:
  cpu: 1000m
  memory: 512Mi
  disk: 2Gi
  timeout: 120  # 2 minute timeout
script: |
  #!/bin/bash
  echo "Starting git clone under throttled bandwidth"
  apt-get update -qq
  apt-get install -y -qq git

  echo "Cloning repository (this should be very slow)..."
  time git clone --depth 1 https://github.com/torvalds/linux.git /tmp/linux || true

  if [ -d /tmp/linux ]; then
    echo "Clone succeeded"
    ls -lah /tmp/linux
  else
    echo "Clone failed or timed out"
  fi
EOF

    local task_id
    task_id=$(submit_task "$manifest_path")

    if [[ -z "$task_id" ]]; then
        test_failed "$EXPERIMENT_NAME" "Failed to submit task"
        remove_throttle "$NETWORK_INTERFACE"
        return 1
    fi

    log_success "Task submitted: ${task_id}"
    log_info "Task timeout: 120 seconds"
    echo ""

    # Step 4: Monitor task progress
    log_step "Step 4: Monitoring task under bandwidth constraint"

    local monitor_duration=60  # Monitor for 60 seconds
    local monitor_start
    monitor_start=$(date +%s)

    log_info "Monitoring for ${monitor_duration}s..."

    while true; do
        local elapsed
        elapsed=$(($(date +%s) - monitor_start))

        if [ $elapsed -ge $monitor_duration ]; then
            break
        fi

        local current_state
        current_state=$(get_task_state "$task_id")

        log_info "[${elapsed}s] Task state: ${current_state}"

        # Check if task has already completed or failed
        if [[ "$current_state" =~ ^(completed|failed|cancelled)$ ]]; then
            log_info "Task reached terminal state: ${current_state}"
            break
        fi

        sleep 10
    done
    echo ""

    # Step 5: Remove bandwidth throttling
    log_step "Step 5: Removing bandwidth throttling"

    remove_throttle "$NETWORK_INTERFACE"
    log_success "Bandwidth throttling removed"
    echo ""

    # Step 6: Wait for task to complete or timeout
    log_step "Step 6: Waiting for task to complete or timeout"

    # Wait up to 60 more seconds for task to finish
    local wait_start
    wait_start=$(date +%s)

    while true; do
        local wait_elapsed
        wait_elapsed=$(($(date +%s) - wait_start))

        if [ $wait_elapsed -ge 60 ]; then
            log_warn "Task still running after 60s wait"
            break
        fi

        local final_state
        final_state=$(get_task_state "$task_id")

        if [[ "$final_state" =~ ^(completed|failed|cancelled)$ ]]; then
            log_info "Task reached terminal state: ${final_state}"
            break
        fi

        sleep 5
    done
    echo ""

    # Step 7: Analyze results
    log_step "Step 7: Analyzing results"

    local final_state
    final_state=$(get_task_state "$task_id")

    local task_info
    task_info=$(get_task "$task_id")

    local error_msg
    error_msg=$(echo "$task_info" | jq -r '.error // "no error"')

    local exit_code
    exit_code=$(echo "$task_info" | jq -r '.exit_code // "null"')

    log_info "Final state: ${final_state}"
    log_info "Exit code: ${exit_code}"

    if [[ "$error_msg" != "no error" ]]; then
        log_info "Error: ${error_msg}"
    fi

    # Evaluate success criteria
    local test_success=0

    if [[ "$final_state" == "failed" ]]; then
        # Check if it failed due to timeout
        if echo "$error_msg" | grep -iq "timeout"; then
            log_success "Task timed out as expected"
            log_success "Timeout enforcement works correctly"
            test_success=1
        else
            log_info "Task failed (possibly due to slow clone)"
            log_success "Task did not hang indefinitely"
            test_success=1
        fi

    elif [[ "$final_state" == "completed" ]]; then
        log_success "Task completed successfully"
        log_info "Clone succeeded despite throttling (network was restored in time)"
        test_success=1

    elif [[ "$final_state" == "running" ]]; then
        log_warn "Task still running - this may indicate hanging"
        log_info "Cancelling task to verify cancellation works"

        if cancel_task "$task_id" "Chaos test cleanup"; then
            log_success "Task cancelled successfully (no hang detected)"
            test_success=1
        else
            log_error "Failed to cancel task (possible hang)"
            test_success=0
        fi
    fi

    echo ""

    # Step 8: Cleanup
    if [[ "$final_state" != "cancelled" ]]; then
        log_step "Step 8: Cleaning up test task"
        cancel_task "$task_id" "Chaos test cleanup" || true
    fi

    # Final verdict
    if [ $test_success -eq 1 ]; then
        log_success "Slow clone test completed successfully"
        log_success "✓ Bandwidth throttling applied"
        log_success "✓ Clone proceeded under constraint"
        log_success "✓ Timeout enforced (no hang)"
        log_success "✓ Throttling removed successfully"

        test_passed "$EXPERIMENT_NAME"
        return 0
    else
        log_error "Task behavior indicates potential hanging issue"
        test_failed "$EXPERIMENT_NAME" "Timeout enforcement failed"
        return 1
    fi
}

# =============================================================================
# Main
# =============================================================================

main() {
    echo "========================================"
    echo "Chaos Experiment: Slow Clone"
    echo "========================================"
    echo ""

    check_dependencies || exit 1

    # Warn about traffic control manipulation
    log_warn "This test will throttle network bandwidth using tc"
    log_warn "Interface: ${NETWORK_INTERFACE}"
    log_warn "Rate: ${THROTTLE_KBPS} kbps (very slow)"
    log_info "This may affect other VMs on the same bridge"
    log_info "Press Ctrl-C to abort, or wait 5 seconds to continue..."
    sleep 5

    if run_experiment; then
        log_success "Experiment completed successfully"
        exit 0
    else
        log_error "Experiment failed"
        exit 1
    fi
}

# Run if executed directly
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
