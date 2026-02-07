#!/bin/bash
# Run all chaos experiments
#
# This orchestrator script runs all chaos experiments in sequence
# and reports a summary of results.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

# =============================================================================
# Configuration
# =============================================================================

# Experiments to run (in order)
EXPERIMENTS=(
    "chaos-server-kill.sh"
    "chaos-storage-fill.sh"
    "chaos-vm-kill.sh"
    "chaos-network-partition.sh"
    "chaos-slow-clone.sh"
)

# Experiment descriptions
declare -A EXPERIMENT_DESCRIPTIONS=(
    ["chaos-server-kill.sh"]="Server Kill - Tests server crash recovery"
    ["chaos-storage-fill.sh"]="Storage Fill - Tests storage exhaustion handling"
    ["chaos-vm-kill.sh"]="VM Kill - Tests VM failure detection"
    ["chaos-network-partition.sh"]="Network Partition - Tests network failure handling"
    ["chaos-slow-clone.sh"]="Slow Clone - Tests timeout enforcement with throttling"
)

# Results tracking
declare -A EXPERIMENT_RESULTS=()
TOTAL_EXPERIMENTS=0
PASSED_EXPERIMENTS=0
FAILED_EXPERIMENTS=0
SKIPPED_EXPERIMENTS=0

# =============================================================================
# Helper Functions
# =============================================================================

print_header() {
    local title="$1"
    local width=60

    echo ""
    echo "$(printf '=%.0s' $(seq 1 $width))"
    echo "$title"
    echo "$(printf '=%.0s' $(seq 1 $width))"
    echo ""
}

print_experiment_header() {
    local experiment="$1"
    local description="${EXPERIMENT_DESCRIPTIONS[$experiment]:-No description}"

    print_header "Running: $experiment"
    log_info "$description"
    echo ""
}

run_single_experiment() {
    local experiment="$1"
    local experiment_path="${SCRIPT_DIR}/${experiment}"

    if [[ ! -f "$experiment_path" ]]; then
        log_error "Experiment not found: ${experiment_path}"
        EXPERIMENT_RESULTS["$experiment"]="MISSING"
        return 1
    fi

    if [[ ! -x "$experiment_path" ]]; then
        log_warn "Experiment not executable, fixing: ${experiment}"
        chmod +x "$experiment_path"
    fi

    print_experiment_header "$experiment"

    # Run the experiment and capture exit code
    local start_time
    start_time=$(date +%s)

    if "$experiment_path"; then
        local end_time
        end_time=$(date +%s)
        local duration=$((end_time - start_time))

        log_success "Experiment PASSED in ${duration}s"
        EXPERIMENT_RESULTS["$experiment"]="PASSED (${duration}s)"
        PASSED_EXPERIMENTS=$((PASSED_EXPERIMENTS + 1))
        return 0
    else
        local end_time
        end_time=$(date +%s)
        local duration=$((end_time - start_time))

        log_error "Experiment FAILED after ${duration}s"
        EXPERIMENT_RESULTS["$experiment"]="FAILED (${duration}s)"
        FAILED_EXPERIMENTS=$((FAILED_EXPERIMENTS + 1))
        return 1
    fi
}

print_summary() {
    print_header "Chaos Testing Summary"

    echo "Experiment Results:"
    echo ""

    local max_name_len=0
    for experiment in "${EXPERIMENTS[@]}"; do
        local len=${#experiment}
        if [ $len -gt $max_name_len ]; then
            max_name_len=$len
        fi
    done

    for experiment in "${EXPERIMENTS[@]}"; do
        local result="${EXPERIMENT_RESULTS[$experiment]:-NOT_RUN}"
        local status_color="$NC"

        if [[ "$result" == PASSED* ]]; then
            status_color="$GREEN"
        elif [[ "$result" == FAILED* ]]; then
            status_color="$RED"
        elif [[ "$result" == SKIPPED* ]]; then
            status_color="$YELLOW"
        else
            status_color="$YELLOW"
        fi

        printf "  %-${max_name_len}s : ${status_color}%s${NC}\n" "$experiment" "$result"
    done

    echo ""
    echo "Statistics:"
    echo "  Total:   ${TOTAL_EXPERIMENTS}"
    echo "  Passed:  ${GREEN}${PASSED_EXPERIMENTS}${NC}"
    echo "  Failed:  ${RED}${FAILED_EXPERIMENTS}${NC}"
    echo "  Skipped: ${YELLOW}${SKIPPED_EXPERIMENTS}${NC}"
    echo ""

    if [ $FAILED_EXPERIMENTS -eq 0 ]; then
        log_success "All chaos experiments passed!"
        return 0
    else
        log_error "${FAILED_EXPERIMENTS} experiment(s) failed"
        return 1
    fi
}

# =============================================================================
# Main Execution
# =============================================================================

main() {
    print_header "Chaos Testing Framework"

    log_info "This suite will run ${#EXPERIMENTS[@]} chaos experiments"
    log_warn "These tests will intentionally disrupt the system"
    log_info "Experiments to run:"

    for experiment in "${EXPERIMENTS[@]}"; do
        local description="${EXPERIMENT_DESCRIPTIONS[$experiment]:-No description}"
        echo "  - ${experiment}: ${description}"
    done

    echo ""
    log_info "Press Ctrl-C to abort, or wait 10 seconds to continue..."
    sleep 10

    # Verify prerequisites once
    print_header "Verifying Prerequisites"
    verify_prerequisites || {
        log_error "Prerequisites not met, aborting"
        exit 1
    }

    # Run each experiment
    TOTAL_EXPERIMENTS=${#EXPERIMENTS[@]}

    for experiment in "${EXPERIMENTS[@]}"; do
        run_single_experiment "$experiment" || true

        # Add a pause between experiments
        if [[ "$experiment" != "${EXPERIMENTS[-1]}" ]]; then
            log_info "Waiting 5 seconds before next experiment..."
            sleep 5
        fi
    done

    # Print summary and exit with appropriate code
    print_summary
}

# Handle command line arguments
case "${1:-}" in
    --help|-h)
        echo "Usage: $0 [OPTIONS]"
        echo ""
        echo "Run all chaos experiments in sequence"
        echo ""
        echo "Options:"
        echo "  --help, -h     Show this help message"
        echo "  --list, -l     List available experiments"
        echo ""
        echo "Experiments:"
        for experiment in "${EXPERIMENTS[@]}"; do
            echo "  - ${experiment}"
        done
        exit 0
        ;;
    --list|-l)
        echo "Available experiments:"
        for experiment in "${EXPERIMENTS[@]}"; do
            description="${EXPERIMENT_DESCRIPTIONS[$experiment]:-No description}"
            echo "  ${experiment}"
            echo "    ${description}"
        done
        exit 0
        ;;
esac

# Run main if executed directly
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
