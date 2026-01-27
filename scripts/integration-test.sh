#!/bin/bash
# Integration test script for agentic-sandbox
# Tests sandbox creation, execution, and cleanup

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEST_TIMEOUT=60
SANDBOX_MANAGER_PORT=8080
MANAGER_PID=""

# Cleanup function
cleanup() {
    echo -e "\n${YELLOW}Cleaning up...${NC}"

    # Stop sandbox-manager if running
    if [ -n "$MANAGER_PID" ]; then
        echo "Stopping sandbox-manager (PID: $MANAGER_PID)"
        kill "$MANAGER_PID" 2>/dev/null || true
        wait "$MANAGER_PID" 2>/dev/null || true
    fi

    # Clean up test containers
    echo "Removing test containers..."
    docker ps -a --filter "name=sandbox-test-" -q | xargs -r docker rm -f 2>/dev/null || true

    # Clean up test networks
    echo "Removing test networks..."
    docker network ls --filter "name=sandbox-test-" -q | xargs -r docker network rm 2>/dev/null || true

    echo -e "${GREEN}Cleanup complete${NC}"
}

# Set up cleanup trap
trap cleanup EXIT INT TERM

# Log functions
log_info() {
    echo -e "${GREEN}[INFO]${NC} $*"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $*"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $*"
}

# Test functions
test_binary_exists() {
    log_info "Testing binary existence..."

    if [ ! -f "${PROJECT_ROOT}/bin/sandbox-manager" ]; then
        log_error "sandbox-manager binary not found"
        return 1
    fi

    if [ ! -f "${PROJECT_ROOT}/bin/sandbox-cli" ]; then
        log_error "sandbox-cli binary not found"
        return 1
    fi

    log_info "Binaries found"
    return 0
}

test_docker_images() {
    log_info "Testing Docker images..."

    if ! docker image inspect agentic-sandbox-base:latest &>/dev/null; then
        log_error "Base image not found"
        return 1
    fi

    if ! docker image inspect agentic-sandbox-test:latest &>/dev/null; then
        log_error "Test image not found"
        return 1
    fi

    log_info "Docker images found"
    return 0
}

test_manager_startup() {
    log_info "Testing sandbox-manager startup..."

    # Start sandbox-manager in background
    "${PROJECT_ROOT}/bin/sandbox-manager" &
    MANAGER_PID=$!

    # Wait for manager to be ready
    local attempts=0
    local max_attempts=30

    while [ $attempts -lt $max_attempts ]; do
        if curl -sf "http://localhost:${SANDBOX_MANAGER_PORT}/health" &>/dev/null; then
            log_info "Sandbox-manager is ready"
            return 0
        fi

        # Check if process is still running
        if ! kill -0 "$MANAGER_PID" 2>/dev/null; then
            log_error "Sandbox-manager process died"
            return 1
        fi

        sleep 1
        ((attempts++))
    done

    log_error "Sandbox-manager failed to start within ${max_attempts} seconds"
    return 1
}

test_sandbox_creation() {
    log_info "Testing sandbox creation..."

    local response
    response=$(curl -sf -X POST \
        "http://localhost:${SANDBOX_MANAGER_PORT}/api/v1/sandboxes" \
        -H "Content-Type: application/json" \
        -d '{
            "name": "test-sandbox-001",
            "runtime": "docker",
            "image": "agentic-sandbox-test:latest",
            "resources": {
                "cpu": "1",
                "memory": "512m"
            }
        }')

    if [ $? -ne 0 ]; then
        log_error "Failed to create sandbox"
        return 1
    fi

    local sandbox_id
    sandbox_id=$(echo "$response" | jq -r '.id')

    if [ -z "$sandbox_id" ] || [ "$sandbox_id" = "null" ]; then
        log_error "Invalid sandbox ID received"
        return 1
    fi

    log_info "Sandbox created: $sandbox_id"
    echo "$sandbox_id" > /tmp/test-sandbox-id
    return 0
}

test_sandbox_execution() {
    log_info "Testing command execution in sandbox..."

    local sandbox_id
    sandbox_id=$(cat /tmp/test-sandbox-id)

    local response
    response=$(curl -sf -X POST \
        "http://localhost:${SANDBOX_MANAGER_PORT}/api/v1/sandboxes/${sandbox_id}/exec" \
        -H "Content-Type: application/json" \
        -d '{
            "command": ["echo", "Hello from sandbox"],
            "timeout": 10
        }')

    if [ $? -ne 0 ]; then
        log_error "Failed to execute command"
        return 1
    fi

    local output
    output=$(echo "$response" | jq -r '.stdout')

    if [ "$output" != "Hello from sandbox" ]; then
        log_error "Unexpected output: $output"
        return 1
    fi

    log_info "Command executed successfully"
    return 0
}

test_sandbox_isolation() {
    log_info "Testing sandbox isolation..."

    local sandbox_id
    sandbox_id=$(cat /tmp/test-sandbox-id)

    # Test network isolation (should fail to reach external services if isolated)
    local response
    response=$(curl -sf -X POST \
        "http://localhost:${SANDBOX_MANAGER_PORT}/api/v1/sandboxes/${sandbox_id}/exec" \
        -H "Content-Type: application/json" \
        -d '{
            "command": ["id", "-u"],
            "timeout": 5
        }')

    local uid
    uid=$(echo "$response" | jq -r '.stdout' | tr -d '\n')

    if [ "$uid" = "0" ]; then
        log_error "Sandbox is running as root (UID: $uid)"
        return 1
    fi

    log_info "Sandbox is running as non-root user (UID: $uid)"
    return 0
}

test_sandbox_cleanup() {
    log_info "Testing sandbox cleanup..."

    local sandbox_id
    sandbox_id=$(cat /tmp/test-sandbox-id)

    curl -sf -X DELETE \
        "http://localhost:${SANDBOX_MANAGER_PORT}/api/v1/sandboxes/${sandbox_id}"

    if [ $? -ne 0 ]; then
        log_error "Failed to delete sandbox"
        return 1
    fi

    # Verify sandbox is gone
    sleep 2
    if docker ps -a --filter "name=${sandbox_id}" --format '{{.Names}}' | grep -q "${sandbox_id}"; then
        log_error "Sandbox container still exists"
        return 1
    fi

    log_info "Sandbox cleaned up successfully"
    rm -f /tmp/test-sandbox-id
    return 0
}

# Main test runner
main() {
    log_info "Starting integration tests"
    log_info "Project root: $PROJECT_ROOT"

    local failed=0

    # Run tests in order
    test_binary_exists || ((failed++))
    test_docker_images || ((failed++))

    # Only proceed with API tests if basic checks pass
    if [ $failed -eq 0 ]; then
        test_manager_startup || ((failed++))

        if [ $failed -eq 0 ]; then
            test_sandbox_creation || ((failed++))
            test_sandbox_execution || ((failed++))
            test_sandbox_isolation || ((failed++))
            test_sandbox_cleanup || ((failed++))
        fi
    fi

    # Report results
    echo ""
    echo "================================"
    if [ $failed -eq 0 ]; then
        echo -e "${GREEN}All tests passed!${NC}"
        return 0
    else
        echo -e "${RED}$failed test(s) failed${NC}"
        return 1
    fi
}

# Run main
main "$@"
