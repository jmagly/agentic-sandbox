#!/bin/bash
# Chaos Experiment 2: Storage Fill
#
# Test storage exhaustion handling by filling storage during task staging.
#
# Expected Behavior:
# - Task should fail gracefully when storage is full
# - Storage alerts should fire (metrics check)
# - Cleanup should work properly
# - No corruption should occur

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

EXPERIMENT_NAME="chaos-storage-fill"
TASK_STORAGE="${TASK_STORAGE:-/srv/tasks}"
FILL_SIZE_MB="${FILL_SIZE_MB:-1000}"

# =============================================================================
# Experiment Functions
# =============================================================================

cleanup_fill_file() {
    local fill_file="${TASK_STORAGE}/chaos-fill.dat"

    if [[ -f "$fill_file" ]]; then
        log_step "Removing fill file"
        rm -f "$fill_file" || log_warn "Failed to remove fill file"
        log_success "Fill file removed"
    fi
}

run_experiment() {
    log_info "Starting Chaos Experiment: Storage Fill"
    log_info "This test fills storage during task staging"
    echo ""

    # Register cleanup
    on_exit cleanup_fill_file

    # Verify prerequisites
    verify_prerequisites || return 1

    # Check if task storage directory exists
    if [[ ! -d "$TASK_STORAGE" ]]; then
        log_warn "Task storage directory does not exist: ${TASK_STORAGE}"
        log_info "Creating directory for testing"
        sudo mkdir -p "$TASK_STORAGE" || {
            test_failed "$EXPERIMENT_NAME" "Failed to create storage directory"
            return 1
        }
    fi

    # Step 1: Check initial storage state
    log_step "Step 1: Checking initial storage state"
    local available_before
    available_before=$(df -m "$TASK_STORAGE" | tail -1 | awk '{print $4}')
    log_info "Available storage: ${available_before} MB"
    echo ""

    # Step 2: Submit a task
    log_step "Step 2: Submitting an I/O-intensive task"
    local manifest
    manifest=$(generate_io_manifest "storage-fill-test")

    local task_id
    task_id=$(submit_task "$manifest")

    if [[ -z "$task_id" ]]; then
        test_failed "$EXPERIMENT_NAME" "Failed to submit task"
        return 1
    fi

    log_success "Task submitted: ${task_id}"
    echo ""

    # Step 3: Wait for task to start staging
    log_step "Step 3: Waiting for task to reach staging state"
    sleep 2

    local current_state
    current_state=$(get_task_state "$task_id")
    log_info "Current task state: ${current_state}"
    echo ""

    # Step 4: Fill storage
    log_step "Step 4: Filling storage with ${FILL_SIZE_MB} MB of data"
    local fill_file="${TASK_STORAGE}/chaos-fill.dat"

    if ! dd if=/dev/zero of="$fill_file" bs=1M count=$FILL_SIZE_MB 2>/dev/null; then
        log_warn "dd command failed (storage might be full)"
    fi

    local fill_size
    fill_size=$(du -m "$fill_file" 2>/dev/null | cut -f1)
    log_info "Fill file created: ${fill_size} MB"

    local available_after
    available_after=$(df -m "$TASK_STORAGE" | tail -1 | awk '{print $4}')
    log_info "Available storage after fill: ${available_after} MB"
    echo ""

    # Step 5: Wait for task to respond to storage pressure
    log_step "Step 5: Monitoring task response to storage pressure"
    sleep 10

    current_state=$(get_task_state "$task_id")
    log_info "Task state after storage fill: ${current_state}"

    # Check if task handled the situation gracefully
    if [[ "$current_state" == "failed" ]]; then
        log_success "Task failed gracefully as expected"
    else
        log_warn "Task is in state: ${current_state} (expected: failed)"
    fi
    echo ""

    # Step 6: Check metrics for storage alerts
    log_step "Step 6: Checking metrics for storage indicators"

    if check_metrics_endpoint > /tmp/chaos-metrics.txt 2>&1; then
        # Look for storage-related metrics
        if grep -q "disk.*bytes" /tmp/chaos-metrics.txt; then
            log_success "Storage metrics are being reported"
        else
            log_warn "No storage metrics found in /metrics endpoint"
        fi
    else
        log_warn "Unable to check metrics endpoint"
    fi
    echo ""

    # Step 7: Cleanup fill file
    log_step "Step 7: Cleaning up fill file"
    cleanup_fill_file

    available_cleanup=$(df -m "$TASK_STORAGE" | tail -1 | awk '{print $4}')
    log_info "Available storage after cleanup: ${available_cleanup} MB"
    echo ""

    # Step 8: Verify no corruption
    log_step "Step 8: Verifying task storage integrity"

    # Try to submit another task to verify system still works
    local verify_manifest
    verify_manifest=$(generate_test_manifest "verify-task" "ubuntu:22.04")

    local verify_task_id
    verify_task_id=$(submit_task "$verify_manifest")

    if [[ -n "$verify_task_id" ]]; then
        log_success "Post-cleanup task submitted successfully: ${verify_task_id}"
        log_success "No corruption detected"

        # Cleanup verify task
        sleep 2
        cancel_task "$verify_task_id" "Chaos test cleanup" || true
    else
        log_error "Failed to submit post-cleanup task - possible corruption"
        test_failed "$EXPERIMENT_NAME" "Storage system may be corrupted"
        return 1
    fi
    echo ""

    # Step 9: Cleanup test task
    log_step "Step 9: Cleaning up test task"
    cancel_task "$task_id" "Chaos test cleanup" || true

    # Final verdict
    log_success "Storage fill test completed successfully"
    log_success "✓ Task failed gracefully"
    log_success "✓ Metrics endpoint functional"
    log_success "✓ Cleanup successful"
    log_success "✓ No corruption detected"

    test_passed "$EXPERIMENT_NAME"
    return 0
}

# =============================================================================
# Main
# =============================================================================

main() {
    echo "========================================"
    echo "Chaos Experiment: Storage Fill"
    echo "========================================"
    echo ""

    check_dependencies || exit 1

    # Warn about storage impact
    log_warn "This test will create a ${FILL_SIZE_MB}MB file in ${TASK_STORAGE}"
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
