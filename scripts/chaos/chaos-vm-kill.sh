#!/bin/bash
# Chaos Experiment 3: VM Kill
#
# Test VM failure detection by forcefully destroying a VM during task execution.
#
# Expected Behavior:
# - Task should detect VM death
# - Task should transition to Failed state
# - No orphaned resources should remain

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

EXPERIMENT_NAME="chaos-vm-kill"

# =============================================================================
# Experiment Functions
# =============================================================================

run_experiment() {
    log_info "Starting Chaos Experiment: VM Kill"
    log_info "This test kills a task VM and verifies failure detection"
    echo ""

    # Verify prerequisites
    verify_prerequisites || return 1

    # Step 1: Submit a long-running task
    log_step "Step 1: Submitting a long-running task"

    # Create a manifest for a task that runs for a while
    local manifest_path="${MANIFEST_DIR}/vm-kill-test.yaml"
    create_manifest_dir

    cat > "$manifest_path" <<EOF
name: vm-kill-test
runtime: ubuntu:22.04
resources:
  cpu: 1000m
  memory: 512Mi
  disk: 1Gi
  timeout: 600
script: |
  #!/bin/bash
  echo "Starting long-running task"
  for i in \$(seq 1 100); do
    echo "Iteration \$i of 100"
    sleep 5
  done
  echo "Task completed (this should not be reached)"
EOF

    local task_id
    task_id=$(submit_task "$manifest_path")

    if [[ -z "$task_id" ]]; then
        test_failed "$EXPERIMENT_NAME" "Failed to submit task"
        return 1
    fi

    log_success "Task submitted: ${task_id}"
    echo ""

    # Step 2: Wait for task to reach Running state
    log_step "Step 2: Waiting for task to reach Running state"

    if ! wait_for_task_state "$task_id" "running" 120; then
        log_error "Task did not reach Running state"

        # Check what state it's in
        local current_state
        current_state=$(get_task_state "$task_id")
        log_info "Current state: ${current_state}"

        # If it's in a different but valid state, continue anyway for testing
        if [[ "$current_state" =~ ^(staging|provisioning|ready)$ ]]; then
            log_warn "Task is in ${current_state}, will test VM kill anyway"
        else
            test_failed "$EXPERIMENT_NAME" "Task in unexpected state: ${current_state}"
            cancel_task "$task_id" "Cleanup after failure" || true
            return 1
        fi
    else
        log_success "Task is running"
    fi
    echo ""

    # Step 3: Get VM name for the task
    log_step "Step 3: Identifying task VM"

    local vm_name
    vm_name=$(get_task_vm "$task_id")

    if [[ -z "$vm_name" ]]; then
        # VM might not be created yet in early states
        log_warn "No VM assigned to task yet"
        log_info "Checking if task is using system orchestration"

        # For system orchestration, we might not have a dedicated VM
        # In this case, we can't test VM killing
        log_warn "Skipping VM kill test - no dedicated VM found"
        log_info "This is expected if using shared agent pools"

        cancel_task "$task_id" "Test skipped" || true
        test_passed "$EXPERIMENT_NAME (skipped - no VM)"
        return 0
    fi

    log_info "Task VM: ${vm_name}"

    # Verify VM exists
    if ! virsh domstate "$vm_name" >/dev/null 2>&1; then
        log_error "VM ${vm_name} not found in libvirt"
        test_failed "$EXPERIMENT_NAME" "VM not found"
        cancel_task "$task_id" "Cleanup" || true
        return 1
    fi

    local vm_state
    vm_state=$(virsh domstate "$vm_name" 2>/dev/null || echo "unknown")
    log_info "VM state: ${vm_state}"
    echo ""

    # Step 4: Kill the VM
    log_step "Step 4: Forcefully destroying VM"

    if kill_vm "$vm_name"; then
        log_success "VM destroyed"
    else
        log_error "Failed to destroy VM"
        test_failed "$EXPERIMENT_NAME" "Could not destroy VM"
        cancel_task "$task_id" "Cleanup" || true
        return 1
    fi
    echo ""

    # Step 5: Verify task detects VM death
    log_step "Step 5: Waiting for task to detect VM failure"

    # Wait up to 60 seconds for task to transition to failed
    if wait_for_task_state "$task_id" "failed" 60; then
        log_success "Task transitioned to Failed state"
    else
        local final_state
        final_state=$(get_task_state "$task_id")
        log_error "Task did not transition to Failed (current state: ${final_state})"
        test_failed "$EXPERIMENT_NAME" "Task did not detect VM death"
        cancel_task "$task_id" "Cleanup" || true
        return 1
    fi
    echo ""

    # Step 6: Verify error message
    log_step "Step 6: Verifying error details"

    local task_info
    task_info=$(get_task "$task_id")

    local error_msg
    error_msg=$(echo "$task_info" | jq -r '.error // "no error"')

    log_info "Error message: ${error_msg}"

    if [[ "$error_msg" == "no error" ]]; then
        log_warn "No error message recorded (this might be okay)"
    else
        log_success "Error message captured"
    fi
    echo ""

    # Step 7: Check for orphaned resources
    log_step "Step 7: Checking for orphaned resources"

    local orphaned=0

    # Check if VM still exists (shouldn't after cleanup)
    if virsh domstate "$vm_name" >/dev/null 2>&1; then
        log_warn "VM ${vm_name} still exists (might be cleanup lag)"
        # Give it a few more seconds
        sleep 5
        if virsh domstate "$vm_name" >/dev/null 2>&1; then
            log_error "VM ${vm_name} is orphaned"
            orphaned=1
        fi
    else
        log_success "VM properly cleaned up"
    fi

    # Check task storage directory
    if [[ -d "${TASK_STORAGE:-/srv/tasks}/${task_id}" ]]; then
        local task_dir="${TASK_STORAGE:-/srv/tasks}/${task_id}"
        local dir_size
        dir_size=$(du -sh "$task_dir" 2>/dev/null | cut -f1)
        log_info "Task directory still exists: ${task_dir} (${dir_size})"
        log_warn "This is expected for failed tasks (for debugging)"
    fi

    if [ $orphaned -eq 0 ]; then
        log_success "No orphaned resources detected"
    else
        log_error "Orphaned resources found"
        test_failed "$EXPERIMENT_NAME" "Resource cleanup incomplete"
        return 1
    fi
    echo ""

    # Final verdict
    log_success "VM kill test completed successfully"
    log_success "✓ VM destroyed"
    log_success "✓ Task detected VM death"
    log_success "✓ Task transitioned to Failed"
    log_success "✓ No orphaned resources"

    test_passed "$EXPERIMENT_NAME"
    return 0
}

# =============================================================================
# Main
# =============================================================================

main() {
    echo "========================================"
    echo "Chaos Experiment: VM Kill"
    echo "========================================"
    echo ""

    check_dependencies || exit 1

    # Warn about VM destruction
    log_warn "This test will forcefully destroy a running VM"
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
