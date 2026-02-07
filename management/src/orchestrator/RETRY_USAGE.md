# Retry Module Usage Guide

This guide demonstrates how to use the retry logic with exponential backoff in the task orchestrator.

## Overview

The retry module provides:
- **RetryPolicy**: Configurable retry behavior with exponential backoff
- **RetryExecutor**: Executes async operations with automatic retry logic
- **Retryable trait**: Classifies errors as retryable or permanent
- **RetryError**: Wraps operation errors with retry context

## Predefined Policies

Three predefined policies are available for common operations:

```rust
use crate::orchestrator::{RetryPolicy, RetryExecutor};

// Git clone operations: 3 attempts, 5s initial delay, 60s max
let policy = &RetryPolicy::GIT_CLONE;

// VM provisioning: 2 attempts, 10s initial delay, 30s max
let policy = &RetryPolicy::VM_PROVISION;

// SSH connections: 5 attempts, 2s initial delay, 30s max
let policy = &RetryPolicy::SSH_CONNECT;
```

## Custom Policies

Create custom policies for specific needs:

```rust
let policy = RetryPolicy {
    max_attempts: 4,
    initial_delay: Duration::from_secs(3),
    max_delay: Duration::from_secs(45),
    multiplier: 1.5,
    jitter: true,  // Add ±15% randomization
};
```

## Error Classification

### Option 1: Implement Retryable Trait

For custom error types, implement the `Retryable` trait:

```rust
use crate::orchestrator::Retryable;

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("VM provisioning failed: {0}")]
    ProvisionFailed(String),

    #[error("VM not ready - no IP assigned")]
    VmNotReady,
}

impl Retryable for ExecutorError {
    fn is_retryable(&self) -> bool {
        match self {
            // Network/transient errors are retryable
            ExecutorError::CommandFailed(msg) => {
                msg.contains("timeout") ||
                msg.contains("connection refused") ||
                msg.contains("temporarily unavailable")
            }
            // VM provisioning might succeed on retry
            ExecutorError::ProvisionFailed(_) => true,
            // VM not ready is retryable (might be starting up)
            ExecutorError::VmNotReady => true,
        }
    }
}
```

### Option 2: Use Heuristic Classification

For errors that don't implement `Retryable`, use `execute_heuristic`:

```rust
// Automatically classifies based on error message patterns
let result = RetryExecutor::execute_heuristic(
    &RetryPolicy::GIT_CLONE,
    "git_clone",
    || async {
        // Operation that returns standard error types
        perform_git_clone().await
    },
)
.await;
```

Heuristic classification treats these as retryable:
- timeout, connection refused, connection reset
- temporarily unavailable, network unreachable
- DNS errors
- Rate limit (429), Service unavailable (503), Gateway timeout (504)

And these as permanent:
- 404 Not Found, 401 Unauthorized, 403 Forbidden
- Invalid, parse, malformed

## Integration Examples

### Example 1: Git Clone with Retry

```rust
use crate::orchestrator::{RetryPolicy, RetryExecutor, Retryable};

async fn clone_repository(url: &str, branch: &str, path: &Path) -> Result<(), ExecutorError> {
    RetryExecutor::execute_heuristic(
        &RetryPolicy::GIT_CLONE,
        "git_clone",
        || async {
            let output = Command::new("git")
                .args(&["clone", "--depth", "1", "--branch", branch, url, &path.to_string_lossy()])
                .output()
                .await
                .map_err(|e| ExecutorError::CommandFailed(format!("git clone: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ExecutorError::CommandFailed(format!("git clone failed: {}", stderr)));
            }

            Ok(())
        },
    )
    .await
    .map_err(|retry_err| match retry_err {
        RetryError::ExhaustedRetries { attempts, last_error } => {
            error!("Git clone failed after {} attempts: {}", attempts, last_error);
            last_error
        }
        RetryError::Permanent(err) => {
            error!("Git clone failed with permanent error: {}", err);
            err
        }
    })
}
```

### Example 2: VM Provisioning with Retry

```rust
async fn provision_vm_with_retry(&self, vm_name: &str, config: &VmConfig) -> Result<VmInfo, ExecutorError> {
    RetryExecutor::execute(
        &RetryPolicy::VM_PROVISION,
        "vm_provision",
        || async {
            self.provision_vm_internal(vm_name, config).await
        },
    )
    .await
    .map_err(|retry_err| match retry_err {
        RetryError::ExhaustedRetries { attempts, last_error } => {
            error!("VM provisioning failed after {} attempts", attempts);
            last_error
        }
        RetryError::Permanent(err) => {
            error!("VM provisioning failed permanently: {}", err);
            err
        }
    })
}
```

### Example 3: SSH Connection with Retry

```rust
async fn wait_for_ssh(&self, ip: &str, ssh_key: &Path) -> Result<(), ExecutorError> {
    RetryExecutor::execute(
        &RetryPolicy::SSH_CONNECT,
        "ssh_connect",
        || async {
            let output = Command::new("ssh")
                .args(&[
                    "-i", &ssh_key.to_string_lossy(),
                    "-o", "StrictHostKeyChecking=no",
                    "-o", "ConnectTimeout=5",
                    &format!("agent@{}", ip),
                    "echo", "ready"
                ])
                .output()
                .await
                .map_err(|e| ExecutorError::CommandFailed(format!("SSH: {}", e)))?;

            if !output.status.success() {
                return Err(ExecutorError::VmNotReady);
            }

            Ok(())
        },
    )
    .await
    .map_err(|retry_err| match retry_err {
        RetryError::ExhaustedRetries { attempts, last_error } => {
            error!("SSH connection failed after {} attempts", attempts);
            last_error
        }
        RetryError::Permanent(err) => {
            error!("SSH connection failed permanently: {}", err);
            err
        }
    })
}
```

### Example 4: Custom Operation with Custom Policy

```rust
async fn fetch_api_data(&self, url: &str) -> Result<String, ExecutorError> {
    let policy = RetryPolicy {
        max_attempts: 5,
        initial_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(30),
        multiplier: 2.0,
        jitter: true,
    };

    RetryExecutor::execute_heuristic(
        &policy,
        "api_fetch",
        || async {
            let response = reqwest::get(url)
                .await
                .map_err(|e| ExecutorError::CommandFailed(format!("HTTP: {}", e)))?;

            if !response.status().is_success() {
                return Err(ExecutorError::CommandFailed(
                    format!("HTTP {}", response.status())
                ));
            }

            response.text()
                .await
                .map_err(|e| ExecutorError::CommandFailed(format!("Parse: {}", e)))
        },
    )
    .await
    .map_err(|retry_err| match retry_err {
        RetryError::ExhaustedRetries { attempts, last_error } => {
            error!("API fetch failed after {} attempts", attempts);
            last_error
        }
        RetryError::Permanent(err) => err,
    })
}
```

## Logging

The retry executor automatically logs:
- Each attempt (at debug level)
- Failures (at warn level)
- Delay before retry (at debug level)
- Success after retries (at debug level)

Example log output:
```
DEBUG Executing git_clone (attempt 1/3)
WARN  git_clone failed on attempt 1/3: Command failed: connection timeout
DEBUG Retrying git_clone after 5s
DEBUG Executing git_clone (attempt 2/3)
DEBUG git_clone succeeded on attempt 2/3
```

## Best Practices

1. **Choose appropriate policies**: Use predefined policies when possible
2. **Classify errors correctly**: Implement `Retryable` for custom error types
3. **Log retry context**: Extract attempt count from `RetryError::ExhaustedRetries`
4. **Use jitter for distributed systems**: Prevents thundering herd
5. **Set reasonable limits**: Balance reliability vs latency
6. **Combine with timeouts**: Use with `TimeoutEnforcer` for complete control

## Testing

The retry module includes comprehensive unit tests. Run them with:

```bash
cargo test orchestrator::retry
```

Key test scenarios:
- Success on first attempt
- Success after retries
- Exhausted retries
- Permanent errors stop immediately
- Exponential backoff timing
- Jitter randomization
- Policy delay calculations
- Heuristic error classification
