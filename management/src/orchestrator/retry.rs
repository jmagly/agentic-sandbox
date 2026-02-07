//! Retry Logic with Exponential Backoff
//!
//! Provides configurable retry policies with exponential backoff and jitter
//! for transient failure scenarios like network operations and external API calls.

use std::future::Future;
use std::time::Duration;
use tracing::{debug, warn};

/// Retry policy configuration
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of attempts (includes initial attempt)
    pub max_attempts: u32,
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Multiplier for exponential backoff (typically 2.0)
    pub multiplier: f64,
    /// Whether to add random jitter (±15%)
    pub jitter: bool,
}

impl RetryPolicy {
    /// Policy for git clone operations
    /// 3 attempts, 5s initial, 60s max, 2.0 multiplier
    pub const GIT_CLONE: Self = Self {
        max_attempts: 3,
        initial_delay: Duration::from_secs(5),
        max_delay: Duration::from_secs(60),
        multiplier: 2.0,
        jitter: true,
    };

    /// Policy for VM provisioning operations
    /// 2 attempts, 10s initial, 30s max
    pub const VM_PROVISION: Self = Self {
        max_attempts: 2,
        initial_delay: Duration::from_secs(10),
        max_delay: Duration::from_secs(30),
        multiplier: 2.0,
        jitter: false,
    };

    /// Policy for SSH connection attempts
    /// 5 attempts, 2s initial, 30s max
    pub const SSH_CONNECT: Self = Self {
        max_attempts: 5,
        initial_delay: Duration::from_secs(2),
        max_delay: Duration::from_secs(30),
        multiplier: 2.0,
        jitter: true,
    };

    /// Calculate delay for a given attempt number (1-indexed)
    fn calculate_delay(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }

        // Calculate exponential delay: initial_delay * multiplier^(attempt-1)
        let exp = attempt - 1;
        let base_delay_secs = self.initial_delay.as_secs_f64() * self.multiplier.powi(exp as i32);
        let mut delay = Duration::from_secs_f64(base_delay_secs.min(self.max_delay.as_secs_f64()));

        // Apply jitter: ±15% randomization
        if self.jitter {
            let jitter_factor = 1.0 + (rand::random::<f64>() * 0.3 - 0.15); // 0.85 to 1.15
            let jittered_secs = delay.as_secs_f64() * jitter_factor;
            delay = Duration::from_secs_f64(jittered_secs.min(self.max_delay.as_secs_f64()));
        }

        delay
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
            jitter: true,
        }
    }
}

/// Error wrapper that includes retry context
#[derive(Debug, thiserror::Error)]
pub enum RetryError<E: std::error::Error> {
    /// All retry attempts exhausted
    #[error("Operation failed after {attempts} attempts: {last_error}")]
    ExhaustedRetries {
        attempts: u32,
        last_error: E,
    },

    /// Operation failed with non-retryable error
    #[error("Non-retryable error: {0}")]
    Permanent(E),
}

/// Trait for determining if an error is retryable
pub trait Retryable {
    /// Returns true if the error represents a transient failure that should be retried
    fn is_retryable(&self) -> bool;
}

/// Default retryability classification based on error messages
/// This is a heuristic approach for errors that don't implement Retryable
fn is_likely_retryable<E: std::error::Error>(error: &E) -> bool {
    let error_str = error.to_string().to_lowercase();

    // Network-related errors (retryable)
    if error_str.contains("timeout")
        || error_str.contains("connection refused")
        || error_str.contains("connection reset")
        || error_str.contains("temporarily unavailable")
        || error_str.contains("network unreachable")
        || error_str.contains("dns")
        || error_str.contains("rate limit")
        || error_str.contains("429")
        || error_str.contains("503")
        || error_str.contains("504")
    {
        return true;
    }

    // Permanent errors (not retryable)
    if error_str.contains("404")
        || error_str.contains("not found")
        || error_str.contains("401")
        || error_str.contains("unauthorized")
        || error_str.contains("403")
        || error_str.contains("forbidden")
        || error_str.contains("invalid")
        || error_str.contains("parse")
        || error_str.contains("malformed")
    {
        return false;
    }

    // Default: treat unknown errors as retryable
    true
}

/// Retry executor that applies retry policies to async operations
pub struct RetryExecutor;

impl RetryExecutor {
    /// Execute an async operation with retry policy
    ///
    /// # Arguments
    /// * `policy` - Retry policy to apply
    /// * `operation_name` - Name of the operation for logging
    /// * `f` - Async closure to execute
    ///
    /// # Returns
    /// * `Ok(T)` - Operation succeeded
    /// * `Err(RetryError::ExhaustedRetries)` - All retries failed
    /// * `Err(RetryError::Permanent)` - Non-retryable error encountered
    pub async fn execute<F, Fut, T, E>(
        policy: &RetryPolicy,
        operation_name: &str,
        mut f: F,
    ) -> Result<T, RetryError<E>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: std::error::Error + Retryable,
    {
        let mut last_error = None;

        for attempt in 1..=policy.max_attempts {
            debug!(
                "Executing {} (attempt {}/{})",
                operation_name, attempt, policy.max_attempts
            );

            match f().await {
                Ok(result) => {
                    if attempt > 1 {
                        debug!(
                            "{} succeeded on attempt {}/{}",
                            operation_name, attempt, policy.max_attempts
                        );
                    }
                    return Ok(result);
                }
                Err(error) => {
                    // Check if error is retryable
                    if !error.is_retryable() {
                        warn!(
                            "{} failed with non-retryable error: {}",
                            operation_name, error
                        );
                        return Err(RetryError::Permanent(error));
                    }

                    warn!(
                        "{} failed on attempt {}/{}: {}",
                        operation_name, attempt, policy.max_attempts, error
                    );

                    last_error = Some(error);

                    // Don't sleep after the last attempt
                    if attempt < policy.max_attempts {
                        let delay = policy.calculate_delay(attempt);
                        debug!(
                            "Retrying {} after {:?}",
                            operation_name,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        // All attempts exhausted
        Err(RetryError::ExhaustedRetries {
            attempts: policy.max_attempts,
            last_error: last_error.expect("last_error should be set after at least one failure"),
        })
    }

    /// Execute an async operation with retry policy (heuristic retryability)
    ///
    /// This variant uses heuristic error classification for errors that don't
    /// implement the Retryable trait.
    ///
    /// # Arguments
    /// * `policy` - Retry policy to apply
    /// * `operation_name` - Name of the operation for logging
    /// * `f` - Async closure to execute
    ///
    /// # Returns
    /// * `Ok(T)` - Operation succeeded
    /// * `Err(RetryError::ExhaustedRetries)` - All retries failed
    /// * `Err(RetryError::Permanent)` - Non-retryable error encountered
    pub async fn execute_heuristic<F, Fut, T, E>(
        policy: &RetryPolicy,
        operation_name: &str,
        mut f: F,
    ) -> Result<T, RetryError<E>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: std::error::Error,
    {
        let mut last_error = None;

        for attempt in 1..=policy.max_attempts {
            debug!(
                "Executing {} (attempt {}/{})",
                operation_name, attempt, policy.max_attempts
            );

            match f().await {
                Ok(result) => {
                    if attempt > 1 {
                        debug!(
                            "{} succeeded on attempt {}/{}",
                            operation_name, attempt, policy.max_attempts
                        );
                    }
                    return Ok(result);
                }
                Err(error) => {
                    // Use heuristic classification
                    if !is_likely_retryable(&error) {
                        warn!(
                            "{} failed with non-retryable error: {}",
                            operation_name, error
                        );
                        return Err(RetryError::Permanent(error));
                    }

                    warn!(
                        "{} failed on attempt {}/{}: {}",
                        operation_name, attempt, policy.max_attempts, error
                    );

                    last_error = Some(error);

                    // Don't sleep after the last attempt
                    if attempt < policy.max_attempts {
                        let delay = policy.calculate_delay(attempt);
                        debug!(
                            "Retrying {} after {:?}",
                            operation_name,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        // All attempts exhausted
        Err(RetryError::ExhaustedRetries {
            attempts: policy.max_attempts,
            last_error: last_error.expect("last_error should be set after at least one failure"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // Test error type that implements Retryable
    #[derive(Debug, thiserror::Error)]
    enum TestError {
        #[error("Transient network error")]
        NetworkError,

        #[error("Permanent validation error")]
        ValidationError,

        #[error("Timeout")]
        Timeout,

        #[error("Resource not found")]
        NotFound,
    }

    impl Retryable for TestError {
        fn is_retryable(&self) -> bool {
            matches!(self, TestError::NetworkError | TestError::Timeout)
        }
    }

    #[test]
    fn test_retry_policy_delay_calculation() {
        let policy = RetryPolicy {
            max_attempts: 5,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
            jitter: false,
        };

        // Attempt 0 should have no delay
        assert_eq!(policy.calculate_delay(0), Duration::ZERO);

        // Attempt 1: 1s * 2^0 = 1s
        assert_eq!(policy.calculate_delay(1), Duration::from_secs(1));

        // Attempt 2: 1s * 2^1 = 2s
        assert_eq!(policy.calculate_delay(2), Duration::from_secs(2));

        // Attempt 3: 1s * 2^2 = 4s
        assert_eq!(policy.calculate_delay(3), Duration::from_secs(4));

        // Attempt 4: 1s * 2^3 = 8s
        assert_eq!(policy.calculate_delay(4), Duration::from_secs(8));

        // Attempt 5: 1s * 2^4 = 16s
        assert_eq!(policy.calculate_delay(5), Duration::from_secs(16));
    }

    #[test]
    fn test_retry_policy_max_delay_cap() {
        let policy = RetryPolicy {
            max_attempts: 10,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5),
            multiplier: 2.0,
            jitter: false,
        };

        // Attempt 5: 1s * 2^4 = 16s, but capped at 5s
        assert_eq!(policy.calculate_delay(5), Duration::from_secs(5));

        // Attempt 10: Would be huge, but capped at 5s
        assert_eq!(policy.calculate_delay(10), Duration::from_secs(5));
    }

    #[test]
    fn test_retry_policy_jitter() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_secs(10),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
            jitter: true,
        };

        // With jitter, delays should vary by ±15%
        // For 10s base: 8.5s to 11.5s
        let delay = policy.calculate_delay(1);
        assert!(delay >= Duration::from_secs_f64(8.5));
        assert!(delay <= Duration::from_secs_f64(11.5));
    }

    #[test]
    fn test_predefined_policies() {
        // GIT_CLONE
        assert_eq!(RetryPolicy::GIT_CLONE.max_attempts, 3);
        assert_eq!(RetryPolicy::GIT_CLONE.initial_delay, Duration::from_secs(5));
        assert_eq!(RetryPolicy::GIT_CLONE.max_delay, Duration::from_secs(60));
        assert_eq!(RetryPolicy::GIT_CLONE.multiplier, 2.0);
        assert!(RetryPolicy::GIT_CLONE.jitter);

        // VM_PROVISION
        assert_eq!(RetryPolicy::VM_PROVISION.max_attempts, 2);
        assert_eq!(RetryPolicy::VM_PROVISION.initial_delay, Duration::from_secs(10));
        assert_eq!(RetryPolicy::VM_PROVISION.max_delay, Duration::from_secs(30));
        assert!(!RetryPolicy::VM_PROVISION.jitter);

        // SSH_CONNECT
        assert_eq!(RetryPolicy::SSH_CONNECT.max_attempts, 5);
        assert_eq!(RetryPolicy::SSH_CONNECT.initial_delay, Duration::from_secs(2));
        assert_eq!(RetryPolicy::SSH_CONNECT.max_delay, Duration::from_secs(30));
        assert!(RetryPolicy::SSH_CONNECT.jitter);
    }

    #[tokio::test]
    async fn test_retry_executor_success_first_attempt() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            multiplier: 2.0,
            jitter: false,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = RetryExecutor::execute(
            &policy,
            "test_operation",
            || async {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Ok::<_, TestError>(42)
            },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_executor_success_after_retries() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            multiplier: 2.0,
            jitter: false,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = RetryExecutor::execute(
            &policy,
            "test_operation",
            || {
                let counter = counter_clone.clone();
                async move {
                    let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    if attempt < 3 {
                        Err(TestError::NetworkError)
                    } else {
                        Ok(42)
                    }
                }
            },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_executor_exhausted_retries() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            multiplier: 2.0,
            jitter: false,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = RetryExecutor::execute(
            &policy,
            "test_operation",
            || async {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(TestError::NetworkError)
            },
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            RetryError::ExhaustedRetries { attempts, .. } => {
                assert_eq!(attempts, 3);
            }
            _ => panic!("Expected ExhaustedRetries"),
        }
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_executor_permanent_error() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            multiplier: 2.0,
            jitter: false,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = RetryExecutor::execute(
            &policy,
            "test_operation",
            || async {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(TestError::ValidationError)
            },
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            RetryError::Permanent(_) => {}
            _ => panic!("Expected Permanent error"),
        }
        // Should stop after first attempt for permanent errors
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_executor_heuristic_retryable() {
        #[derive(Debug, thiserror::Error)]
        #[error("{0}")]
        struct GenericError(String);

        let policy = RetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            multiplier: 2.0,
            jitter: false,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        // Timeout error should be retryable
        let result = RetryExecutor::execute_heuristic(
            &policy,
            "test_operation",
            || {
                let counter = counter_clone.clone();
                async move {
                    let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    if attempt < 3 {
                        Err(GenericError("Connection timeout".to_string()))
                    } else {
                        Ok::<_, GenericError>(42)
                    }
                }
            },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_executor_heuristic_permanent() {
        #[derive(Debug, thiserror::Error)]
        #[error("{0}")]
        struct GenericError(String);

        let policy = RetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            multiplier: 2.0,
            jitter: false,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        // 404 error should not be retryable
        let result = RetryExecutor::execute_heuristic(
            &policy,
            "test_operation",
            || async {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(GenericError("404 not found".to_string()))
            },
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            RetryError::Permanent(_) => {}
            _ => panic!("Expected Permanent error"),
        }
        // Should stop after first attempt
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_heuristic_classification() {
        #[derive(Debug, thiserror::Error)]
        #[error("{0}")]
        struct E(String);

        // Retryable errors
        assert!(is_likely_retryable(&E("Connection timeout".to_string())));
        assert!(is_likely_retryable(&E("Connection refused".to_string())));
        assert!(is_likely_retryable(&E("Network unreachable".to_string())));
        assert!(is_likely_retryable(&E("Rate limit exceeded (429)".to_string())));
        assert!(is_likely_retryable(&E("Service unavailable (503)".to_string())));
        assert!(is_likely_retryable(&E("Gateway timeout (504)".to_string())));
        assert!(is_likely_retryable(&E("DNS resolution failed".to_string())));

        // Non-retryable errors
        assert!(!is_likely_retryable(&E("404 not found".to_string())));
        assert!(!is_likely_retryable(&E("Resource not found".to_string())));
        assert!(!is_likely_retryable(&E("401 unauthorized".to_string())));
        assert!(!is_likely_retryable(&E("403 forbidden".to_string())));
        assert!(!is_likely_retryable(&E("Invalid input format".to_string())));
        assert!(!is_likely_retryable(&E("Parse error".to_string())));
        assert!(!is_likely_retryable(&E("Malformed request".to_string())));
    }

    #[tokio::test]
    async fn test_retry_timing() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(200),
            multiplier: 2.0,
            jitter: false,
        };

        let start = std::time::Instant::now();

        let _result = RetryExecutor::execute(
            &policy,
            "test_operation",
            || async { Err::<i32, _>(TestError::NetworkError) },
        )
        .await;

        let elapsed = start.elapsed();

        // Should have delays of 50ms and 100ms (no delay after last attempt)
        // Total: ~150ms (with some tolerance for execution time)
        assert!(elapsed >= Duration::from_millis(140));
        assert!(elapsed < Duration::from_millis(300));
    }
}
