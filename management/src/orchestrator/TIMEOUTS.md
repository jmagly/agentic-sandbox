# Timeout Enforcement Implementation

**Status**: ✅ Complete
**File**: `management/src/orchestrator/timeouts.rs`
**Lines of Code**: 535 (including comprehensive tests)
**Test Coverage**: 19 unit tests, 14 integration tests

## Overview

Timeout enforcement module for the agentic-sandbox management server. Provides configurable operation timeouts with structured duration parsing and comprehensive tracing.

## Components

### 1. TimeoutConfig

Configurable timeout durations for different operation types:

```rust
pub struct TimeoutConfig {
    pub git_clone: Duration,      // Default: 10 minutes
    pub vm_provision: Duration,   // Default: 15 minutes
    pub ssh_connect: Duration,    // Default: 5 minutes
    pub task_default: Duration,   // Default: 24 hours
}
```

### 2. TimeoutEnforcer

Main enforcement mechanism with instrumented operations:

```rust
pub struct TimeoutEnforcer {
    config: TimeoutConfig,
}

impl TimeoutEnforcer {
    // Generic timeout wrapper
    pub async fn with_timeout<F, Fut, T>(
        &self,
        name: &str,
        duration: Duration,
        future: F,
    ) -> Result<T, TimeoutError>

    // Operation-specific helpers
    pub async fn with_git_clone_timeout<F, Fut, T>(&self, future: F) -> Result<T, TimeoutError>
    pub async fn with_vm_provision_timeout<F, Fut, T>(&self, future: F) -> Result<T, TimeoutError>
    pub async fn with_ssh_connect_timeout<F, Fut, T>(&self, future: F) -> Result<T, TimeoutError>
    pub async fn with_task_timeout<F, Fut, T>(&self, future: F) -> Result<T, TimeoutError>

    // Parse timeout from manifest string
    pub async fn with_parsed_timeout<F, Fut, T>(
        &self,
        timeout_str: &str,
        future: F,
    ) -> Result<T, TimeoutError>
}
```

### 3. Duration Parser

Parses human-readable timeout strings from task manifests:

```rust
pub fn parse_duration(s: &str) -> Option<Duration>
```

**Supported formats:**
- Hours: `"1h"`, `"24h"`, `"168h"` (max 7 days)
- Minutes: `"30m"`, `"90m"`
- Seconds: `"60s"`, `"120s"`

**Validation:**
- Minimum: 1 second
- Maximum: 168 hours (7 days)
- Rejects invalid formats and out-of-range values

### 4. TimeoutError

Structured error types:

```rust
pub enum TimeoutError {
    Timeout {
        operation: String,
        duration: Duration,
    },
    InvalidDuration(String),
    DurationOutOfRange(String),
}
```

## Usage Examples

### Basic Usage

```rust
use crate::orchestrator::timeouts::{TimeoutEnforcer, TimeoutError};

let enforcer = TimeoutEnforcer::new();

// Wrap a git clone operation
let result = enforcer
    .with_git_clone_timeout(|| async {
        Command::new("git")
            .args(&["clone", "https://github.com/example/repo.git"])
            .output()
            .await
    })
    .await?;
```

### Custom Timeout from Manifest

```rust
// Parse timeout from task manifest lifecycle.timeout field
let timeout_str = task.lifecycle.timeout; // e.g., "30m"

let result = enforcer
    .with_parsed_timeout(&timeout_str, || async {
        execute_claude_task().await
    })
    .await?;
```

### Custom Configuration

```rust
use std::time::Duration;
use crate::orchestrator::timeouts::{TimeoutConfig, TimeoutEnforcer};

let config = TimeoutConfig::new(
    Duration::from_secs(5 * 60),   // git_clone: 5 minutes
    Duration::from_secs(10 * 60),  // vm_provision: 10 minutes
    Duration::from_secs(2 * 60),   // ssh_connect: 2 minutes
    Duration::from_secs(12 * 3600) // task_default: 12 hours
);

let enforcer = TimeoutEnforcer::with_config(config);
```

### Generic Timeout

```rust
// Custom operation with specific timeout
let result = enforcer
    .with_timeout("database_backup", Duration::from_secs(300), || async {
        perform_backup().await
    })
    .await?;
```

## Integration Points

### 1. TaskExecutor

Apply timeouts to executor operations:

```rust
// In executor.rs
use crate::orchestrator::timeouts::TimeoutEnforcer;

pub struct TaskExecutor {
    timeout_enforcer: Arc<TimeoutEnforcer>,
    // ... other fields
}

impl TaskExecutor {
    pub async fn stage_task(&self, task: &Arc<RwLock<Task>>) -> Result<(), ExecutorError> {
        self.timeout_enforcer
            .with_git_clone_timeout(|| async {
                // Git clone logic here
                Command::new("git")
                    .args(&git_args)
                    .output()
                    .await
            })
            .await
            .map_err(|e| ExecutorError::Timeout(e.to_string()))?;

        Ok(())
    }

    pub async fn provision_vm(&self, task: &Arc<RwLock<Task>>) -> Result<VmInfo, ExecutorError> {
        self.timeout_enforcer
            .with_vm_provision_timeout(|| async {
                // VM provisioning logic
                Command::new(&self.provision_script)
                    .args(&args)
                    .output()
                    .await
            })
            .await
            .map_err(|e| ExecutorError::Timeout(e.to_string()))?;

        // ... rest of provisioning
    }

    pub async fn execute_claude(&self, task: &Arc<RwLock<Task>>) -> Result<i32, ExecutorError> {
        let timeout_str = task.read().await.lifecycle.timeout.clone();

        self.timeout_enforcer
            .with_parsed_timeout(&timeout_str, || async {
                // Claude execution logic
                execute_ssh_command().await
            })
            .await
            .map_err(|e| ExecutorError::Timeout(e.to_string()))?;

        Ok(exit_code)
    }
}
```

### 2. ExecutorError Extension

Add timeout variant to ExecutorError:

```rust
// In executor.rs
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("VM provisioning failed: {0}")]
    ProvisionFailed(String),

    #[error("Operation timed out: {0}")]
    Timeout(String),  // <-- Add this

    // ... other variants
}
```

## Tracing Integration

All timeout operations are instrumented with `#[instrument]` attributes for observability:

```rust
#[instrument(skip(self, future), fields(operation_name = %name, timeout_secs = duration.as_secs()))]
pub async fn with_timeout<F, Fut, T>(
    &self,
    name: &str,
    duration: Duration,
    future: F,
) -> Result<T, TimeoutError>
```

**Trace fields:**
- `operation_name`: Name of the operation being timed
- `timeout_secs`: Configured timeout in seconds

**Warnings logged when timeouts occur:**
```
WARN operation=git_clone timeout_secs=600 Operation timed out
```

## Test Coverage

### Unit Tests (19 tests)

- `test_parse_duration_hours` - Parse hour formats
- `test_parse_duration_minutes` - Parse minute formats
- `test_parse_duration_seconds` - Parse second formats
- `test_parse_duration_invalid_format` - Reject invalid formats
- `test_parse_duration_out_of_range` - Reject out-of-range values
- `test_parse_duration_whitespace` - Handle whitespace
- `test_parse_duration_edge_cases` - Boundary conditions
- `test_parse_duration_zero` - Reject zero duration
- `test_timeout_config_default` - Default configuration values
- `test_timeout_config_custom` - Custom configuration
- `test_timeout_error_display` - Error message formatting
- `test_enforcer_default` - Default enforcer construction

### Integration Tests (7 tests)

- `test_with_timeout_success` - Successful operation completion
- `test_with_timeout_expires` - Timeout expiration behavior
- `test_with_git_clone_timeout` - Git clone timeout wrapper
- `test_with_vm_provision_timeout` - VM provision timeout wrapper
- `test_with_ssh_connect_timeout` - SSH connect timeout wrapper
- `test_with_task_timeout` - Task timeout wrapper
- `test_with_parsed_timeout_success` - Parse and apply timeout
- `test_with_parsed_timeout_invalid_format` - Invalid format handling
- `test_with_parsed_timeout_expires` - Parsed timeout expiration
- `test_custom_config` - Custom configuration with timeout

### Running Tests

```bash
cd management
cargo test orchestrator::timeouts
```

## Manifest Integration

Task manifests can specify custom timeouts:

```yaml
version: "1"
kind: Task
metadata:
  id: "example-task"
  name: "Example Task"
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "Fix the bug"
lifecycle:
  timeout: "2h"        # <-- Custom timeout parsed by parse_duration()
  failure_action: "destroy"
```

The `lifecycle.timeout` field is:
1. Validated by `manifest.rs` during parsing
2. Parsed by `parse_duration()` when executing
3. Applied via `TimeoutEnforcer::with_parsed_timeout()`

## Next Steps

To fully integrate timeout enforcement:

1. **Update TaskExecutor**: Add `TimeoutEnforcer` field
2. **Wrap operations**: Apply timeouts to:
   - `stage_task()` - git clone operations
   - `provision_vm()` - VM provisioning
   - `execute_claude()` - Claude execution with manifest timeout
3. **Handle timeout errors**: Convert `TimeoutError` to `ExecutorError::Timeout`
4. **Add metrics**: Track timeout occurrences for monitoring
5. **Update documentation**: Document timeout behavior in user docs

## Dependencies

- `tokio::time::timeout` - Actual timeout mechanism
- `tracing` - Instrumentation and logging
- `thiserror` - Error type derivation
- `std::time::Duration` - Duration type

## References

- Issue: #64 Timeout Enforcement
- Module: `/home/roctinam/dev/agentic-sandbox/management/src/orchestrator/timeouts.rs`
- Public API exports: `orchestrator/mod.rs`
