#!/bin/bash
# Chaos Experiment 1: Server Kill
#
# Test server recovery by killing the management server process
# during active task execution.
#
# Expected Behavior:
# - Tasks should recover from checkpoints
# - No task state should be lost
# - Server should restart successfully

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

EXPERIMENT_NAME="chaos-server-kill"
TASK_COUNT=5
MGMT_DIR="${MGMT_DIR:-/home/roctinam/dev/agentic-sandbox/management}"

# =============================================================================
# Experiment Functions
# =============================================================================

run_experiment() {
    log_info "Starting Chaos Experiment: Server Kill"
    log_info "This test kills the management server and verifies recovery"
    echo ""

    # Verify prerequisites
    verify_prerequisites || return 1

    local task_ids=()

    # Step 1: Submit multiple tasks
    log_step "Step 1: Submitting ${TASK_COUNT} tasks"
    for i in $(seq 1 $TASK_COUNT); do
        local manifest
        manifest=$(generate_test_manifest "recovery-test-${i}" "ubuntu:22.04")

        local task_id
        task_id=$(submit_task "$manifest")

        if [[ -z "$task_id" ]]; then
            test_failed "$EXPERIMENT_NAME" "Failed to submit task $i"
            return 1
        fi

        task_ids+=("$task_id")
        log_info "Submitted task ${i}: ${task_id}"
        sleep 0.5
    done

    log_success "All ${TASK_COUNT} tasks submitted"
    echo ""

    # Step 2: Wait for tasks to start processing
    log_step "Step 2: Waiting for tasks to begin processing"
    sleep 5

    # Capture task states before kill
    log_step "Capturing task states before server kill"
    declare -A states_before
    for task_id in "${task_ids[@]}"; do
        local state
        state=$(get_task_state "$task_id")
        states_before["$task_id"]="$state"
        log_info "Task ${task_id}: ${state}"
    done
    echo ""

    # Step 3: Kill the management server
    log_step "Step 3: Killing management server process"
    local server_pid
    server_pid=$(get_mgmt_pid)

    if [[ -z "$server_pid" ]]; then
        test_failed "$EXPERIMENT_NAME" "Management server is not running"
        return 1
    fi

    log_info "Server PID: ${server_pid}"
    kill_mgmt_server 9 || {
        test_failed "$EXPERIMENT_NAME" "Failed to kill server"
        return 1
    }

    log_success "Server killed with signal 9"
    echo ""

    # Step 4: Wait for crash to settle
    log_step "Step 4: Waiting 5 seconds for crash to settle"
    sleep 5

    # Step 5: Restart server
    log_step "Step 5: Restarting management server"
    start_mgmt_server "$MGMT_DIR" || {
        test_failed "$EXPERIMENT_NAME" "Failed to restart server"
        return 1
    }

    log_success "Server restarted successfully"
    echo ""

    # Step 6: Verify task recovery
    log_step "Step 6: Verifying task recovery"
    local recovery_failed=0

    for task_id in "${task_ids[@]}"; do
        local state_before="${states_before[$task_id]}"
        local state_after
        state_after=$(get_task_state "$task_id")

        log_info "Task ${task_id}:"
        log_info "  Before: ${state_before}"
        log_info "  After:  ${state_after}"

        # Verify task still exists and is in a valid state
        if [[ "$state_after" == "unknown" || "$state_after" == "null" ]]; then
            log_error "Task ${task_id} lost after recovery!"
            recovery_failed=1
        fi
    done
    echo ""

    # Step 7: Verify no state loss
    if [ $recovery_failed -eq 0 ]; then
        log_success "All tasks recovered successfully"
        log_success "No task state was lost"
        test_passed "$EXPERIMENT_NAME"
    else
        test_failed "$EXPERIMENT_NAME" "Some tasks lost state during recovery"
        return 1
    fi

    # Cleanup
    log_step "Cleaning up test tasks"
    for task_id in "${task_ids[@]}"; do
        cancel_task "$task_id" "Chaos test cleanup" || true
    done

    return 0
}

# =============================================================================
# Main
# =============================================================================

main() {
    echo "========================================"
    echo "Chaos Experiment: Server Kill"
    echo "========================================"
    echo ""

    check_dependencies || exit 1

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
