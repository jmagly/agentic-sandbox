# Chaos Testing Framework

Chaos engineering experiments for testing the resilience and fault tolerance of the agentic-sandbox management system.

## Overview

This framework implements 5 chaos experiments that intentionally inject failures into the system to verify recovery mechanisms, error handling, and overall system resilience.

## Directory Structure

```
scripts/chaos/
├── README.md                      # This file
├── lib/
│   └── common.sh                  # Shared functions and utilities
├── chaos-server-kill.sh           # Experiment 1: Server crash recovery
├── chaos-storage-fill.sh          # Experiment 2: Storage exhaustion
├── chaos-vm-kill.sh               # Experiment 3: VM failure detection
├── chaos-network-partition.sh     # Experiment 4: Network failure handling
├── chaos-slow-clone.sh            # Experiment 5: Timeout enforcement
└── run-all.sh                     # Orchestrator to run all experiments
```

## Prerequisites

### System Requirements

- Management server running on `localhost:8122`
- libvirt/QEMU for VM management
- iptables for network manipulation (requires sudo)
- tc (traffic control) for bandwidth throttling (requires sudo)

### Dependencies

```bash
# Install required packages
sudo apt-get install curl jq iproute2 iptables

# Verify libvirt is installed
virsh version

# Configure passwordless sudo for chaos operations (optional but recommended)
echo "$USER ALL=(ALL) NOPASSWD: /usr/sbin/iptables, /usr/sbin/tc" | sudo tee /etc/sudoers.d/chaos-testing
```

### Server Setup

Start the management server before running experiments:

```bash
cd management
./dev.sh
```

Verify the server is running:

```bash
curl http://localhost:8122/api/v1/health
```

## Experiments

### 1. Server Kill (`chaos-server-kill.sh`)

**Objective**: Test server recovery and task persistence after unexpected shutdown.

**Procedure**:
1. Submit 5 concurrent tasks
2. Kill management server with `kill -9`
3. Wait 5 seconds
4. Restart server
5. Verify all tasks recovered from checkpoints

**Success Criteria**:
- No task state lost
- All tasks resume from last checkpoint
- Server restarts successfully

**Run**:
```bash
./chaos-server-kill.sh
```

### 2. Storage Fill (`chaos-storage-fill.sh`)

**Objective**: Test graceful handling of storage exhaustion.

**Procedure**:
1. Submit a task
2. Fill storage with 1GB file during staging
3. Verify task fails gracefully
4. Check metrics endpoint for storage alerts
5. Remove fill file and verify cleanup

**Success Criteria**:
- Task fails gracefully (no crash)
- Storage alerts visible in metrics
- System cleanup works
- No file system corruption

**Run**:
```bash
./chaos-storage-fill.sh
```

**Configuration**:
```bash
FILL_SIZE_MB=1000 ./chaos-storage-fill.sh  # Adjust fill size
TASK_STORAGE=/custom/path ./chaos-storage-fill.sh  # Custom storage location
```

### 3. VM Kill (`chaos-vm-kill.sh`)

**Objective**: Test VM failure detection and cleanup.

**Procedure**:
1. Submit a long-running task
2. Wait for VM to start (Running state)
3. Forcefully destroy VM with `virsh destroy`
4. Verify task detects VM death
5. Verify task transitions to Failed state
6. Check for orphaned resources

**Success Criteria**:
- Task detects VM failure within 60 seconds
- Task transitions to Failed state
- No orphaned VMs or resources
- Error message captured

**Run**:
```bash
./chaos-vm-kill.sh
```

### 4. Network Partition (`chaos-network-partition.sh`)

**Objective**: Test network failure handling and retry logic.

**Procedure**:
1. Submit task that clones from GitHub
2. Block network with iptables: `iptables -A OUTPUT -d github.com -j DROP`
3. Monitor task behavior under network partition
4. Restore network access
5. Verify task timeout or recovery

**Success Criteria**:
- Git clone retries on network failure
- Task eventually times out or succeeds after recovery
- Retry logic observable
- Network restoration works

**Run**:
```bash
./chaos-network-partition.sh
```

**Configuration**:
```bash
BLOCK_TARGET=example.com ./chaos-network-partition.sh  # Custom target
BLOCK_DURATION=60 ./chaos-network-partition.sh  # Longer partition
```

**Requirements**: Passwordless sudo for iptables

### 5. Slow Clone (`chaos-slow-clone.sh`)

**Objective**: Test timeout enforcement with bandwidth throttling.

**Procedure**:
1. Apply bandwidth throttling with `tc qdisc` (50 kbps)
2. Submit task that clones large repository
3. Monitor clone progress under throttling
4. Verify timeout enforcement
5. Remove throttling

**Success Criteria**:
- Clone proceeds slowly under throttling
- Timeout enforced if clone takes too long
- Task does not hang indefinitely
- Throttling removal works

**Run**:
```bash
./chaos-slow-clone.sh
```

**Configuration**:
```bash
THROTTLE_KBPS=100 ./chaos-slow-clone.sh  # Adjust bandwidth
NETWORK_INTERFACE=virbr1 ./chaos-slow-clone.sh  # Different interface
```

**Requirements**: Passwordless sudo for tc command

## Running All Experiments

The `run-all.sh` orchestrator runs all experiments in sequence:

```bash
./run-all.sh
```

**Features**:
- Sequential execution with pauses between experiments
- Comprehensive results summary
- Pass/fail tracking
- Duration reporting

**Output Example**:
```
========================================
Chaos Testing Summary
========================================
Experiment Results:

  chaos-server-kill.sh        : PASSED (45s)
  chaos-storage-fill.sh       : PASSED (32s)
  chaos-vm-kill.sh            : PASSED (78s)
  chaos-network-partition.sh  : PASSED (56s)
  chaos-slow-clone.sh         : PASSED (102s)

Statistics:
  Total:   5
  Passed:  5
  Failed:  0
  Skipped: 0

[SUCCESS] All chaos experiments passed!
```

**Options**:
```bash
./run-all.sh --help    # Show help
./run-all.sh --list    # List experiments
```

## Common Library (`lib/common.sh`)

The common library provides shared functions used by all experiments:

### Logging Functions
- `log_info()` - Informational messages
- `log_success()` - Success messages
- `log_error()` - Error messages
- `log_warn()` - Warning messages
- `log_step()` - Experiment step headers

### API Functions
- `check_server_health()` - Verify server is responding
- `submit_task(manifest_path)` - Submit task, returns task_id
- `get_task_state(task_id)` - Get current task state
- `get_task(task_id)` - Get full task details JSON
- `wait_for_task_state(task_id, state, timeout)` - Wait for specific state
- `cancel_task(task_id, reason)` - Cancel a task
- `list_tasks(state_filter)` - List all tasks

### Metrics Functions
- `check_metrics_endpoint()` - Fetch Prometheus metrics
- `check_metric_value(metric, pattern)` - Verify metric value

### Process Management
- `get_mgmt_pid()` - Get management server PID
- `kill_mgmt_server(signal)` - Kill management server
- `start_mgmt_server(dir)` - Start management server

### VM Management
- `get_task_vm(task_id)` - Get VM name for task
- `kill_vm(vm_name)` - Destroy a VM

### Test Utilities
- `verify_prerequisites()` - Check all prerequisites
- `check_dependencies()` - Verify required commands exist
- `test_passed(name)` - Mark test as passed
- `test_failed(name, reason)` - Mark test as failed
- `print_test_summary()` - Print test results

### Cleanup
- `on_exit(command)` - Register cleanup command
- `cleanup_on_exit()` - Execute cleanup tasks

### Manifest Generators
- `generate_test_manifest(name, runtime)` - Simple test task
- `generate_clone_manifest(name, repo)` - Git clone task
- `generate_io_manifest(name)` - I/O intensive task

## Configuration

### Environment Variables

All experiments support these common variables:

```bash
# API endpoint configuration
MGMT_API=http://localhost:8122/api/v1
MGMT_HOST=localhost
MGMT_PORT=8122

# Manifest storage
MANIFEST_DIR=/tmp/chaos-manifests

# Server directory
MGMT_DIR=/home/roctinam/dev/agentic-sandbox/management
```

### Experiment-Specific Variables

See individual experiment sections above for experiment-specific configuration options.

## Development

### Writing New Experiments

Create a new experiment script following this template:

```bash
#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"

EXPERIMENT_NAME="chaos-my-experiment"

run_experiment() {
    log_info "Starting experiment: ${EXPERIMENT_NAME}"

    verify_prerequisites || return 1

    # Step 1: Setup
    log_step "Step 1: Setting up test"

    # Your test logic here

    # Final verdict
    test_passed "$EXPERIMENT_NAME"
    return 0
}

main() {
    echo "========================================"
    echo "Chaos Experiment: My Experiment"
    echo "========================================"

    if run_experiment; then
        exit 0
    else
        exit 1
    fi
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
```

### Testing Individual Experiments

```bash
# Make script executable
chmod +x chaos-my-experiment.sh

# Run in dry-run mode (if supported)
DRY_RUN=1 ./chaos-my-experiment.sh

# Run with verbose output
set -x
./chaos-my-experiment.sh
```

### Adding to Orchestrator

Edit `run-all.sh` and add your experiment:

```bash
EXPERIMENTS=(
    "chaos-server-kill.sh"
    "chaos-storage-fill.sh"
    "chaos-vm-kill.sh"
    "chaos-network-partition.sh"
    "chaos-slow-clone.sh"
    "chaos-my-experiment.sh"  # Add here
)

declare -A EXPERIMENT_DESCRIPTIONS=(
    # ...
    ["chaos-my-experiment.sh"]="My Experiment - Description"
)
```

## Troubleshooting

### Server Not Responding

```bash
# Check if server is running
curl http://localhost:8122/api/health

# Start server manually
cd management
./dev.sh

# Check logs
tail -f /tmp/mgmt-server.log
```

### Permission Denied

```bash
# Make scripts executable
chmod +x chaos-*.sh run-all.sh

# Fix common.sh
chmod +x lib/common.sh
```

### Sudo Required

Some experiments require sudo for:
- iptables manipulation (network partition)
- tc command (bandwidth throttling)
- Storage operations (if task storage requires elevation)

Configure passwordless sudo:

```bash
# Edit sudoers
sudo visudo -f /etc/sudoers.d/chaos-testing

# Add these lines
your_user ALL=(ALL) NOPASSWD: /usr/sbin/iptables
your_user ALL=(ALL) NOPASSWD: /usr/sbin/tc
```

### VM Not Found

If VM kill experiment fails:

```bash
# List available VMs
virsh list --all

# Check task orchestration mode
curl http://localhost:8122/api/v1/tasks | jq '.tasks[].vm_name'
```

### Network Interface Issues

Find correct network interface:

```bash
# List interfaces
ip link show

# Common libvirt interfaces
# - virbr0 (default NAT bridge)
# - virbr1 (custom bridges)

# Set in environment
NETWORK_INTERFACE=virbr0 ./chaos-slow-clone.sh
```

## Best Practices

1. **Run on Test Systems**: Only run chaos tests on non-production systems
2. **Monitor Impact**: Watch system resources during experiments
3. **Sequential Execution**: Run experiments one at a time to avoid interference
4. **Cleanup**: Always verify cleanup completed successfully
5. **Logs**: Keep experiment logs for debugging: `./run-all.sh 2>&1 | tee chaos.log`
6. **Baseline**: Establish baseline behavior before running chaos tests
7. **Isolation**: Use dedicated test infrastructure when possible

## CI/CD Integration

### GitHub Actions

```yaml
name: Chaos Testing

on:
  schedule:
    - cron: '0 2 * * *'  # Daily at 2 AM
  workflow_dispatch:

jobs:
  chaos:
    runs-on: self-hosted
    steps:
      - uses: actions/checkout@v3

      - name: Start Management Server
        run: |
          cd management
          ./dev.sh &
          sleep 10

      - name: Run Chaos Tests
        run: |
          cd scripts/chaos
          ./run-all.sh

      - name: Upload Results
        if: always()
        uses: actions/upload-artifact@v3
        with:
          name: chaos-results
          path: /tmp/chaos-*.log
```

### Jenkins

```groovy
pipeline {
    agent any

    triggers {
        cron('H 2 * * *')
    }

    stages {
        stage('Setup') {
            steps {
                sh 'cd management && ./dev.sh &'
                sh 'sleep 10'
            }
        }

        stage('Chaos Tests') {
            steps {
                sh 'cd scripts/chaos && ./run-all.sh'
            }
        }
    }

    post {
        always {
            archiveArtifacts artifacts: 'scripts/chaos/*.log'
        }
    }
}
```

## Metrics and Observability

Chaos experiments interact with the Prometheus metrics endpoint:

```bash
# View all metrics
curl http://localhost:8122/metrics

# Check specific metrics during experiments
curl http://localhost:8122/metrics | grep -E 'task_state|agent_status|disk_bytes'
```

## References

- [Principles of Chaos Engineering](https://principlesofchaos.org/)
- [Chaos Toolkit](https://chaostoolkit.org/)
- [Netflix Chaos Monkey](https://netflix.github.io/chaosmonkey/)
- [Litmus Chaos](https://litmuschaos.io/)

## Contributing

To add new chaos experiments:

1. Create experiment script in `scripts/chaos/`
2. Follow naming convention: `chaos-*.sh`
3. Use common library functions
4. Add documentation to this README
5. Update `run-all.sh` orchestrator
6. Test thoroughly before committing

## License

This chaos testing framework is part of the agentic-sandbox project and follows the same license.
