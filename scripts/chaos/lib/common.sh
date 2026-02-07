#!/bin/bash
# Common functions for chaos testing experiments

# Terminal color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Management server endpoint
MGMT_API="${MGMT_API:-http://localhost:8122/api/v1}"
MGMT_HOST="${MGMT_HOST:-localhost}"
MGMT_PORT="${MGMT_PORT:-8122}"

# Test manifest directory
MANIFEST_DIR="${MANIFEST_DIR:-/tmp/chaos-manifests}"

# =============================================================================
# Logging Functions
# =============================================================================

log_info() {
    echo -e "${BLUE}[INFO]${NC} $*"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $*"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $*"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $*"
}

log_step() {
    echo -e "${BLUE}==>${NC} $*"
}

# =============================================================================
# HTTP Helper Functions
# =============================================================================

# Make an API call with error handling
api_call() {
    local method="$1"
    local endpoint="$2"
    local data="${3:-}"
    local response

    if [[ -n "$data" ]]; then
        response=$(curl -s -X "$method" \
            -H "Content-Type: application/json" \
            -d "$data" \
            "${MGMT_API}${endpoint}" 2>&1)
    else
        response=$(curl -s -X "$method" \
            "${MGMT_API}${endpoint}" 2>&1)
    fi

    echo "$response"
}

# Check if management server is reachable
check_server_health() {
    local max_attempts="${1:-30}"
    local attempt=0

    log_step "Checking management server health..."

    while [ $attempt -lt $max_attempts ]; do
        if curl -s -f "${MGMT_API%/v1}/health" > /dev/null 2>&1; then
            log_success "Management server is healthy"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done

    log_error "Management server is not responding after ${max_attempts}s"
    return 1
}

# =============================================================================
# Task Management Functions
# =============================================================================

# Submit a task and return its ID
# Args: manifest_path
# Returns: task_id on stdout
submit_task() {
    local manifest_path="$1"

    if [[ ! -f "$manifest_path" ]]; then
        log_error "Manifest not found: $manifest_path"
        return 1
    fi

    local manifest_yaml
    manifest_yaml=$(cat "$manifest_path")

    local payload
    payload=$(jq -n --arg yaml "$manifest_yaml" '{manifest_yaml: $yaml}')

    local response
    response=$(api_call POST "/tasks" "$payload")

    # Check if task was accepted
    local accepted
    accepted=$(echo "$response" | jq -r '.accepted // false')

    if [[ "$accepted" != "true" ]]; then
        local error
        error=$(echo "$response" | jq -r '.error // "Unknown error"')
        log_error "Task submission failed: $error"
        return 1
    fi

    local task_id
    task_id=$(echo "$response" | jq -r '.task_id')

    if [[ -z "$task_id" || "$task_id" == "null" ]]; then
        log_error "Failed to extract task ID from response"
        return 1
    fi

    echo "$task_id"
}

# Get task state
# Args: task_id
# Returns: state string on stdout
get_task_state() {
    local task_id="$1"

    local response
    response=$(api_call GET "/tasks/${task_id}")

    local state
    state=$(echo "$response" | jq -r '.state // "unknown"')

    echo "$state"
}

# Get full task details
# Args: task_id
# Returns: JSON task object on stdout
get_task() {
    local task_id="$1"
    api_call GET "/tasks/${task_id}"
}

# Wait for task to reach expected state
# Args: task_id expected_state timeout_seconds
# Returns: 0 if state reached, 1 on timeout
wait_for_task_state() {
    local task_id="$1"
    local expected_state="$2"
    local timeout="${3:-300}"

    local elapsed=0
    local check_interval=2

    log_step "Waiting for task ${task_id} to reach state: ${expected_state}"

    while [ $elapsed -lt $timeout ]; do
        local current_state
        current_state=$(get_task_state "$task_id")

        if [[ "$current_state" == "$expected_state" ]]; then
            log_success "Task ${task_id} reached state: ${expected_state}"
            return 0
        fi

        # Check for terminal failure states
        if [[ "$current_state" == "failed" && "$expected_state" != "failed" ]]; then
            log_error "Task ${task_id} failed unexpectedly"
            return 1
        fi

        if [[ "$current_state" == "cancelled" && "$expected_state" != "cancelled" ]]; then
            log_error "Task ${task_id} was cancelled"
            return 1
        fi

        sleep $check_interval
        elapsed=$((elapsed + check_interval))
    done

    log_error "Timeout waiting for task ${task_id} to reach ${expected_state}"
    return 1
}

# Cancel a task
# Args: task_id [reason]
cancel_task() {
    local task_id="$1"
    local reason="${2:-Chaos test cleanup}"

    local payload
    payload=$(jq -n --arg reason "$reason" '{reason: $reason}')

    local response
    response=$(api_call DELETE "/tasks/${task_id}" "$payload")

    local success
    success=$(echo "$response" | jq -r '.success // false')

    if [[ "$success" == "true" ]]; then
        log_success "Task ${task_id} cancelled"
        return 0
    else
        local error
        error=$(echo "$response" | jq -r '.error // "Unknown error"')
        log_error "Failed to cancel task: $error"
        return 1
    fi
}

# List all tasks with optional state filter
# Args: [state_filter]
# Returns: JSON array of tasks
list_tasks() {
    local state_filter="${1:-}"
    local endpoint="/tasks"

    if [[ -n "$state_filter" ]]; then
        endpoint="${endpoint}?state=${state_filter}"
    fi

    api_call GET "$endpoint"
}

# =============================================================================
# Metrics and Monitoring
# =============================================================================

# Check Prometheus metrics endpoint
check_metrics_endpoint() {
    log_step "Checking metrics endpoint..."

    local metrics
    metrics=$(curl -s "http://${MGMT_HOST}:${MGMT_PORT}/metrics")

    if [[ -z "$metrics" ]]; then
        log_error "Metrics endpoint returned empty response"
        return 1
    fi

    log_success "Metrics endpoint is responding"
    echo "$metrics"
}

# Check if a specific metric exists and matches pattern
# Args: metric_name pattern
check_metric_value() {
    local metric_name="$1"
    local pattern="$2"

    local metrics
    metrics=$(check_metrics_endpoint)

    if echo "$metrics" | grep -q "${metric_name}.*${pattern}"; then
        log_success "Metric ${metric_name} matches pattern: ${pattern}"
        return 0
    else
        log_error "Metric ${metric_name} does not match pattern: ${pattern}"
        return 1
    fi
}

# =============================================================================
# Process Management
# =============================================================================

# Get management server PID
get_mgmt_pid() {
    pgrep -f "agentic-mgmt|management.*server" | head -n1
}

# Kill management server
kill_mgmt_server() {
    local signal="${1:-9}"

    local pid
    pid=$(get_mgmt_pid)

    if [[ -z "$pid" ]]; then
        log_warn "Management server is not running"
        return 1
    fi

    log_step "Killing management server (PID: ${pid}) with signal ${signal}"
    kill -"$signal" "$pid"

    # Wait for process to die
    local attempts=0
    while kill -0 "$pid" 2>/dev/null && [ $attempts -lt 10 ]; do
        sleep 0.5
        attempts=$((attempts + 1))
    done

    if kill -0 "$pid" 2>/dev/null; then
        log_error "Failed to kill management server"
        return 1
    fi

    log_success "Management server killed"
    return 0
}

# Start management server
start_mgmt_server() {
    local server_dir="${1:-/home/roctinam/dev/agentic-sandbox/management}"

    log_step "Starting management server from ${server_dir}"

    if [[ ! -d "$server_dir" ]]; then
        log_error "Server directory not found: ${server_dir}"
        return 1
    fi

    cd "$server_dir" || return 1

    # Start in background
    ./dev.sh restart > /tmp/mgmt-server.log 2>&1 &

    # Wait for server to be ready
    sleep 3
    check_server_health 30
}

# =============================================================================
# VM Management
# =============================================================================

# Get VM name for task
# Args: task_id
get_task_vm() {
    local task_id="$1"

    local response
    response=$(get_task "$task_id")

    local vm_name
    vm_name=$(echo "$response" | jq -r '.vm_name // empty')

    echo "$vm_name"
}

# Kill a VM
# Args: vm_name
kill_vm() {
    local vm_name="$1"

    if [[ -z "$vm_name" ]]; then
        log_error "VM name is required"
        return 1
    fi

    log_step "Destroying VM: ${vm_name}"

    if virsh domstate "$vm_name" >/dev/null 2>&1; then
        virsh destroy "$vm_name" >/dev/null 2>&1
        log_success "VM ${vm_name} destroyed"
        return 0
    else
        log_warn "VM ${vm_name} not found or not running"
        return 1
    fi
}

# =============================================================================
# Cleanup Functions
# =============================================================================

# Track cleanup tasks
declare -a CLEANUP_TASKS=()

# Register cleanup task
on_exit() {
    CLEANUP_TASKS+=("$1")
}

# Execute cleanup tasks
cleanup_on_exit() {
    log_info "Running cleanup tasks..."

    for task in "${CLEANUP_TASKS[@]}"; do
        eval "$task" || true
    done

    log_success "Cleanup complete"
}

# Set trap for cleanup
trap cleanup_on_exit EXIT INT TERM

# =============================================================================
# Test Manifest Generators
# =============================================================================

# Create test manifest directory
create_manifest_dir() {
    mkdir -p "$MANIFEST_DIR"
}

# Generate a simple test manifest
# Args: name [runtime]
generate_test_manifest() {
    local name="${1:-test-task}"
    local runtime="${2:-python:3.11}"

    create_manifest_dir

    local manifest_path="${MANIFEST_DIR}/${name}.yaml"

    cat > "$manifest_path" <<EOF
name: ${name}
runtime: ${runtime}
resources:
  cpu: 1000m
  memory: 512Mi
  disk: 1Gi
  timeout: 600
script: |
  #!/bin/bash
  echo "Test task: ${name}"
  echo "Runtime: ${runtime}"
  sleep 30
  echo "Task completed"
EOF

    echo "$manifest_path"
}

# Generate a manifest that clones from GitHub
# Args: name repo_url
generate_clone_manifest() {
    local name="${1:-clone-task}"
    local repo_url="${2:-https://github.com/octocat/Hello-World.git}"

    create_manifest_dir

    local manifest_path="${MANIFEST_DIR}/${name}.yaml"

    cat > "$manifest_path" <<EOF
name: ${name}
runtime: ubuntu:22.04
resources:
  cpu: 1000m
  memory: 512Mi
  disk: 2Gi
  timeout: 300
script: |
  #!/bin/bash
  apt-get update -qq
  apt-get install -y -qq git
  git clone ${repo_url} /tmp/repo
  cd /tmp/repo
  ls -la
  echo "Clone successful"
EOF

    echo "$manifest_path"
}

# Generate a manifest with heavy I/O
# Args: name
generate_io_manifest() {
    local name="${1:-io-task}"

    create_manifest_dir

    local manifest_path="${MANIFEST_DIR}/${name}.yaml"

    cat > "$manifest_path" <<EOF
name: ${name}
runtime: ubuntu:22.04
resources:
  cpu: 1000m
  memory: 512Mi
  disk: 5Gi
  timeout: 600
script: |
  #!/bin/bash
  echo "Performing I/O operations..."
  dd if=/dev/zero of=/tmp/test.dat bs=1M count=100
  ls -lh /tmp/test.dat
  rm /tmp/test.dat
  echo "I/O test completed"
EOF

    echo "$manifest_path"
}

# =============================================================================
# Validation Functions
# =============================================================================

# Check if required commands exist
check_dependencies() {
    local deps=("curl" "jq" "virsh")
    local missing=()

    for dep in "${deps[@]}"; do
        if ! command -v "$dep" >/dev/null 2>&1; then
            missing+=("$dep")
        fi
    done

    if [ ${#missing[@]} -gt 0 ]; then
        log_error "Missing required dependencies: ${missing[*]}"
        return 1
    fi

    return 0
}

# Verify test prerequisites
verify_prerequisites() {
    log_info "Verifying test prerequisites..."

    # Check dependencies
    check_dependencies || return 1

    # Check server health
    check_server_health 10 || {
        log_error "Management server is not running"
        log_info "Start the server with: cd management && ./dev.sh"
        return 1
    }

    log_success "All prerequisites verified"
    return 0
}

# =============================================================================
# Test Result Tracking
# =============================================================================

# Global test result counters
TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

# Mark test as passed
test_passed() {
    local test_name="$1"
    TESTS_PASSED=$((TESTS_PASSED + 1))
    TESTS_RUN=$((TESTS_RUN + 1))
    log_success "TEST PASSED: ${test_name}"
}

# Mark test as failed
test_failed() {
    local test_name="$1"
    local reason="${2:-Unknown reason}"
    TESTS_FAILED=$((TESTS_FAILED + 1))
    TESTS_RUN=$((TESTS_RUN + 1))
    log_error "TEST FAILED: ${test_name} - ${reason}"
}

# Print test summary
print_test_summary() {
    echo ""
    echo "========================================"
    echo "Test Summary"
    echo "========================================"
    echo "Total:  ${TESTS_RUN}"
    echo "Passed: ${TESTS_PASSED}"
    echo "Failed: ${TESTS_FAILED}"
    echo "========================================"

    if [ $TESTS_FAILED -eq 0 ]; then
        log_success "All tests passed!"
        return 0
    else
        log_error "${TESTS_FAILED} test(s) failed"
        return 1
    fi
}
