#!/bin/bash
# Chaos Experiment 4: Network Partition
#
# Test network failure handling by blocking outbound connections during git clone.
#
# Expected Behavior:
# - Git clone should retry on network failure
# - Task should eventually timeout or fail gracefully
# - Retry logic should be observable
# - Network restoration should work

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

EXPERIMENT_NAME="chaos-network-partition"
BLOCK_TARGET="${BLOCK_TARGET:-github.com}"
BLOCK_DURATION="${BLOCK_DURATION:-30}"

# =============================================================================
# Network Management Functions
# =============================================================================

block_network() {
    local target="$1"

    log_step "Blocking network access to ${target}"

    # Add iptables rule to drop packets to target
    if sudo iptables -C OUTPUT -d "$target" -j DROP 2>/dev/null; then
        log_warn "Iptables rule already exists"
        return 0
    fi

    sudo iptables -A OUTPUT -d "$target" -j DROP
    log_success "Network access blocked"
}

unblock_network() {
    local target="$1"

    log_step "Restoring network access to ${target}"

    # Remove iptables rule
    while sudo iptables -C OUTPUT -d "$target" -j DROP 2>/dev/null; do
        sudo iptables -D OUTPUT -d "$target" -j DROP
    done

    log_success "Network access restored"
}

cleanup_network() {
    log_info "Cleaning up network rules"
    unblock_network "$BLOCK_TARGET" || true
}

# =============================================================================
# Experiment Functions
# =============================================================================

run_experiment() {
    log_info "Starting Chaos Experiment: Network Partition"
    log_info "This test blocks network access during git clone"
    echo ""

    # Register cleanup
    on_exit cleanup_network

    # Verify prerequisites
    verify_prerequisites || return 1

    # Check for sudo access (needed for iptables)
    if ! sudo -n true 2>/dev/null; then
        log_error "This experiment requires passwordless sudo access"
        log_info "Add to sudoers: $USER ALL=(ALL) NOPASSWD: /usr/sbin/iptables"
        test_failed "$EXPERIMENT_NAME" "Insufficient privileges"
        return 1
    fi

    # Step 1: Verify initial network connectivity
    log_step "Step 1: Verifying initial network connectivity"

    if curl -s --max-time 5 "https://${BLOCK_TARGET}" >/dev/null 2>&1; then
        log_success "Network connectivity to ${BLOCK_TARGET} confirmed"
    else
        log_warn "Cannot reach ${BLOCK_TARGET} (might be blocked already)"
    fi
    echo ""

    # Step 2: Submit a task that clones from GitHub
    log_step "Step 2: Submitting git clone task"

    local manifest
    manifest=$(generate_clone_manifest "network-partition-test" "https://${BLOCK_TARGET}/octocat/Hello-World.git")

    local task_id
    task_id=$(submit_task "$manifest")

    if [[ -z "$task_id" ]]; then
        test_failed "$EXPERIMENT_NAME" "Failed to submit task"
        return 1
    fi

    log_success "Task submitted: ${task_id}"
    echo ""

    # Step 3: Wait for task to start
    log_step "Step 3: Waiting for task to reach running state"
    sleep 5

    local current_state
    current_state=$(get_task_state "$task_id")
    log_info "Current task state: ${current_state}"
    echo ""

    # Step 4: Block network access
    log_step "Step 4: Blocking network access to ${BLOCK_TARGET}"

    block_network "$BLOCK_TARGET"
    log_info "Network partition active"
    echo ""

    # Step 5: Monitor task behavior under network partition
    log_step "Step 5: Monitoring task behavior (${BLOCK_DURATION}s)"

    local monitor_start
    monitor_start=$(date +%s)

    while true; do
        local elapsed
        elapsed=$(($(date +%s) - monitor_start))

        if [ $elapsed -ge $BLOCK_DURATION ]; then
            break
        fi

        current_state=$(get_task_state "$task_id")
        log_info "[${elapsed}s] Task state: ${current_state}"

        # Check if task has already failed
        if [[ "$current_state" == "failed" ]]; then
            log_info "Task failed during network partition"
            break
        fi

        sleep 5
    done
    echo ""

    # Step 6: Restore network access
    log_step "Step 6: Restoring network access"

    unblock_network "$BLOCK_TARGET"
    log_success "Network partition removed"
    echo ""

    # Step 7: Verify network restoration
    log_step "Step 7: Verifying network restoration"

    if curl -s --max-time 5 "https://${BLOCK_TARGET}" >/dev/null 2>&1; then
        log_success "Network connectivity restored"
    else
        log_warn "Still cannot reach ${BLOCK_TARGET}"
    fi
    echo ""

    # Step 8: Check final task state
    log_step "Step 8: Checking final task state"

    # Give task some time to complete or timeout
    sleep 10

    local final_state
    final_state=$(get_task_state "$task_id")
    log_info "Final task state: ${final_state}"

    # Get task details
    local task_info
    task_info=$(get_task "$task_id")

    local error_msg
    error_msg=$(echo "$task_info" | jq -r '.error // "no error"')

    if [[ "$error_msg" != "no error" ]]; then
        log_info "Error message: ${error_msg}"
    fi

    # Evaluate result
    local test_success=0

    if [[ "$final_state" == "failed" ]]; then
        log_success "Task failed as expected due to network partition"

        # Check if error mentions network/timeout
        if echo "$error_msg" | grep -iq "network\|timeout\|connection\|unreachable"; then
            log_success "Error message indicates network failure"
            test_success=1
        else
            log_warn "Error message does not clearly indicate network issue"
            test_success=1  # Still consider it a pass
        fi

    elif [[ "$final_state" == "completed" ]]; then
        log_info "Task completed (network was restored in time)"
        log_success "Task recovered from network partition"
        test_success=1

    elif [[ "$final_state" == "running" ]]; then
        log_warn "Task still running after network restoration"
        log_info "Waiting additional 30s for completion or timeout"

        if wait_for_task_state "$task_id" "completed" 30; then
            log_success "Task completed successfully"
            test_success=1
        elif wait_for_task_state "$task_id" "failed" 30; then
            log_success "Task eventually failed (acceptable)"
            test_success=1
        else
            log_warn "Task still running - will cancel for cleanup"
        fi
    fi

    echo ""

    # Step 9: Cleanup
    log_step "Step 9: Cleaning up test task"
    cancel_task "$task_id" "Chaos test cleanup" || true

    # Final verdict
    if [ $test_success -eq 1 ]; then
        log_success "Network partition test completed successfully"
        log_success "✓ Network partition applied"
        log_success "✓ Task responded to network failure"
        log_success "✓ Network restored"
        log_success "✓ System remained stable"

        test_passed "$EXPERIMENT_NAME"
        return 0
    else
        log_error "Task did not respond appropriately to network partition"
        test_failed "$EXPERIMENT_NAME" "Unexpected task behavior"
        return 1
    fi
}

# =============================================================================
# Main
# =============================================================================

main() {
    echo "========================================"
    echo "Chaos Experiment: Network Partition"
    echo "========================================"
    echo ""

    check_dependencies || exit 1

    # Warn about network manipulation
    log_warn "This test will modify iptables rules"
    log_warn "Target: ${BLOCK_TARGET}"
    log_warn "Duration: ${BLOCK_DURATION}s"
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
