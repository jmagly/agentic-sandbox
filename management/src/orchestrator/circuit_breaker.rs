//! Circuit Breaker Pattern for Fault Tolerance
//!
//! Prevents cascade failures when external services (GitHub, Claude API) are unavailable
//! by implementing the circuit breaker pattern with three states: Closed, Open, and HalfOpen.
//!
//! A circuit breaker wraps service calls and monitors failures. When failures exceed a threshold,
//! it "opens" the circuit, failing fast without making actual calls. After a timeout, it enters
//! "half-open" state to test if the service has recovered.

use std::future::Future;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - requests pass through
    Closed,
    /// Failing - reject requests immediately (fast-fail)
    Open,
    /// Testing recovery - limited requests allowed
    HalfOpen,
}

impl From<u8> for CircuitState {
    fn from(value: u8) -> Self {
        match value {
            0 => CircuitState::Closed,
            1 => CircuitState::Open,
            2 => CircuitState::HalfOpen,
            _ => CircuitState::Closed, // Default to closed for invalid values
        }
    }
}

impl From<CircuitState> for u8 {
    fn from(state: CircuitState) -> u8 {
        match state {
            CircuitState::Closed => 0,
            CircuitState::Open => 1,
            CircuitState::HalfOpen => 2,
        }
    }
}

/// Configuration for circuit breaker behavior
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures to open the circuit (default: 5)
    pub failure_threshold: u32,
    /// Number of successes in half-open to close the circuit (default: 3)
    pub success_threshold: u32,
    /// Time to wait before transitioning from open to half-open (default: 30s)
    pub timeout: Duration,
    /// Time window for counting failures (default: 60s)
    pub failure_window: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 3,
            timeout: Duration::from_secs(30),
            failure_window: Duration::from_secs(60),
        }
    }
}

impl CircuitBreakerConfig {
    /// Configuration for GitHub API calls
    /// More lenient - GitHub may have transient issues
    pub const GITHUB_API: Self = Self {
        failure_threshold: 3,
        success_threshold: 2,
        timeout: Duration::from_secs(30),
        failure_window: Duration::from_secs(60),
    };

    /// Configuration for Claude API calls
    /// Standard settings for AI service
    pub const CLAUDE_API: Self = Self {
        failure_threshold: 5,
        success_threshold: 3,
        timeout: Duration::from_secs(45),
        failure_window: Duration::from_secs(90),
    };

    /// Configuration for VM provisioning
    /// Less sensitive - VMs take time to provision
    pub const VM_PROVISION: Self = Self {
        failure_threshold: 2,
        success_threshold: 1,
        timeout: Duration::from_secs(60),
        failure_window: Duration::from_secs(120),
    };
}

/// Circuit breaker for preventing cascade failures
pub struct CircuitBreaker {
    name: String,
    state: AtomicU8,
    failure_count: AtomicU32,
    success_count: AtomicU32,
    last_failure: AtomicU64,
    config: CircuitBreakerConfig,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with default configuration
    pub fn new(name: &str) -> Self {
        Self::with_config(name, CircuitBreakerConfig::default())
    }

    /// Create a new circuit breaker with custom configuration
    pub fn with_config(name: &str, config: CircuitBreakerConfig) -> Self {
        debug!(
            "Creating circuit breaker '{}' with failure_threshold={}, success_threshold={}, timeout={:?}",
            name, config.failure_threshold, config.success_threshold, config.timeout
        );

        Self {
            name: name.to_string(),
            state: AtomicU8::new(CircuitState::Closed.into()),
            failure_count: AtomicU32::new(0),
            success_count: AtomicU32::new(0),
            last_failure: AtomicU64::new(0),
            config,
        }
    }

    /// Get current circuit state
    pub fn state(&self) -> CircuitState {
        let state_value = self.state.load(Ordering::Acquire);
        CircuitState::from(state_value)
    }

    /// Check if a request can be executed
    pub fn can_execute(&self) -> bool {
        let current_state = self.state();

        match current_state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if timeout has elapsed to transition to half-open
                let now = Self::current_timestamp();
                let last_failure = self.last_failure.load(Ordering::Acquire);
                let elapsed = now.saturating_sub(last_failure);

                if elapsed >= self.config.timeout.as_secs() {
                    // Transition to half-open
                    self.transition_to_half_open();
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => {
                // In half-open, allow requests but monitor carefully
                true
            }
        }
    }

    /// Record a successful execution
    pub fn record_success(&self) {
        let current_state = self.state();

        match current_state {
            CircuitState::Closed => {
                // Reset failure count on success
                self.failure_count.store(0, Ordering::Release);
            }
            CircuitState::HalfOpen => {
                let success_count = self.success_count.fetch_add(1, Ordering::AcqRel) + 1;

                if success_count >= self.config.success_threshold {
                    // Enough successes - transition back to closed
                    self.transition_to_closed();
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but handle gracefully
                debug!("Received success while circuit is open for '{}'", self.name);
            }
        }
    }

    /// Record a failed execution
    pub fn record_failure(&self) {
        let now = Self::current_timestamp();
        let last_failure = self.last_failure.load(Ordering::Acquire);

        // Check if we're outside the failure window
        if last_failure > 0
            && now.saturating_sub(last_failure) >= self.config.failure_window.as_secs()
        {
            // Reset failure count - old failures don't count
            self.failure_count.store(1, Ordering::Release);
        } else {
            self.failure_count.fetch_add(1, Ordering::AcqRel);
        }

        self.last_failure.store(now, Ordering::Release);

        let current_state = self.state();
        let failure_count = self.failure_count.load(Ordering::Acquire);

        match current_state {
            CircuitState::Closed => {
                if failure_count >= self.config.failure_threshold {
                    self.transition_to_open();
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open immediately goes back to open
                self.transition_to_open();
            }
            CircuitState::Open => {
                // Already open, just update failure time
            }
        }
    }

    /// Execute a function with circuit breaker protection
    pub async fn execute<F, Fut, T, E>(&self, f: F) -> Result<T, CircuitBreakerError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        if !self.can_execute() {
            return Err(CircuitBreakerError::CircuitOpen(format!(
                "Circuit breaker '{}' is open",
                self.name
            )));
        }

        match f().await {
            Ok(result) => {
                self.record_success();
                Ok(result)
            }
            Err(err) => {
                self.record_failure();
                Err(CircuitBreakerError::ServiceError(err))
            }
        }
    }

    /// Reset the circuit breaker to closed state
    pub fn reset(&self) {
        info!("Resetting circuit breaker '{}'", self.name);
        self.state
            .store(CircuitState::Closed.into(), Ordering::Release);
        self.failure_count.store(0, Ordering::Release);
        self.success_count.store(0, Ordering::Release);
        self.last_failure.store(0, Ordering::Release);
    }

    /// Get current Unix timestamp in seconds
    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }

    /// Transition to open state
    fn transition_to_open(&self) {
        warn!(
            "Circuit breaker '{}' opening after {} failures",
            self.name,
            self.failure_count.load(Ordering::Acquire)
        );
        self.state
            .store(CircuitState::Open.into(), Ordering::Release);
        self.success_count.store(0, Ordering::Release);
    }

    /// Transition to half-open state
    fn transition_to_half_open(&self) {
        info!(
            "Circuit breaker '{}' entering half-open state after timeout",
            self.name
        );
        self.state
            .store(CircuitState::HalfOpen.into(), Ordering::Release);
        self.success_count.store(0, Ordering::Release);
    }

    /// Transition to closed state
    fn transition_to_closed(&self) {
        info!(
            "Circuit breaker '{}' closing after {} successful attempts",
            self.name,
            self.success_count.load(Ordering::Acquire)
        );
        self.state
            .store(CircuitState::Closed.into(), Ordering::Release);
        self.failure_count.store(0, Ordering::Release);
        self.success_count.store(0, Ordering::Release);
    }
}

/// Circuit breaker error types
#[derive(Debug, thiserror::Error)]
pub enum CircuitBreakerError<E> {
    /// Circuit is open - request rejected without execution
    #[error("Circuit open: {0}")]
    CircuitOpen(String),

    /// Service call failed
    #[error("Service error: {0}")]
    ServiceError(E),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32};
    use std::sync::Arc;

    // Test error type
    #[derive(Debug, thiserror::Error, Clone)]
    enum TestError {
        #[error("Network error")]
        NetworkError,

        #[error("Timeout")]
        Timeout,
    }

    #[test]
    fn test_circuit_state_conversion() {
        assert_eq!(u8::from(CircuitState::Closed), 0);
        assert_eq!(u8::from(CircuitState::Open), 1);
        assert_eq!(u8::from(CircuitState::HalfOpen), 2);

        assert_eq!(CircuitState::from(0), CircuitState::Closed);
        assert_eq!(CircuitState::from(1), CircuitState::Open);
        assert_eq!(CircuitState::from(2), CircuitState::HalfOpen);
        assert_eq!(CircuitState::from(99), CircuitState::Closed); // Invalid defaults to Closed
    }

    #[test]
    fn test_circuit_breaker_initial_state() {
        let cb = CircuitBreaker::new("test");
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.can_execute());
    }

    #[test]
    fn test_circuit_breaker_with_config() {
        let config = CircuitBreakerConfig {
            failure_threshold: 10,
            success_threshold: 5,
            timeout: Duration::from_secs(60),
            failure_window: Duration::from_secs(120),
        };

        let cb = CircuitBreaker::with_config("test", config.clone());
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.config.failure_threshold, 10);
        assert_eq!(cb.config.success_threshold, 5);
        assert_eq!(cb.config.timeout, Duration::from_secs(60));
        assert_eq!(cb.config.failure_window, Duration::from_secs(120));
    }

    #[test]
    fn test_predefined_configs() {
        // GitHub API config
        assert_eq!(CircuitBreakerConfig::GITHUB_API.failure_threshold, 3);
        assert_eq!(CircuitBreakerConfig::GITHUB_API.success_threshold, 2);

        // Claude API config
        assert_eq!(CircuitBreakerConfig::CLAUDE_API.failure_threshold, 5);
        assert_eq!(CircuitBreakerConfig::CLAUDE_API.success_threshold, 3);

        // VM Provision config
        assert_eq!(CircuitBreakerConfig::VM_PROVISION.failure_threshold, 2);
        assert_eq!(CircuitBreakerConfig::VM_PROVISION.success_threshold, 1);
    }

    #[test]
    fn test_record_success_in_closed_state() {
        let cb = CircuitBreaker::new("test");

        // Record some failures
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.failure_count.load(Ordering::Acquire), 2);

        // Success should reset failure count
        cb.record_success();
        assert_eq!(cb.failure_count.load(Ordering::Acquire), 0);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_transition_to_open_on_failure_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            timeout: Duration::from_secs(30),
            failure_window: Duration::from_secs(60),
        };

        let cb = CircuitBreaker::with_config("test", config);
        assert_eq!(cb.state(), CircuitState::Closed);

        // Record failures up to threshold
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.can_execute());
    }

    #[test]
    fn test_failure_window_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            timeout: Duration::from_secs(1),
            failure_window: Duration::from_secs(1),
        };

        let cb = CircuitBreaker::with_config("test", config);

        // Record one failure
        cb.record_failure();
        assert_eq!(cb.failure_count.load(Ordering::Acquire), 1);

        // Wait for failure window to expire (use 1200ms to be safe)
        std::thread::sleep(Duration::from_millis(1200));

        // Next failure should reset the count
        cb.record_failure();
        assert_eq!(cb.failure_count.load(Ordering::Acquire), 1);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_transition_to_half_open_after_timeout() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            timeout: Duration::from_secs(1),
            failure_window: Duration::from_secs(60),
        };

        let cb = CircuitBreaker::with_config("test", config);

        // Trigger open state
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.can_execute());

        // Wait for timeout
        std::thread::sleep(Duration::from_millis(1100));

        // can_execute should transition to half-open
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_half_open_to_closed_on_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            timeout: Duration::from_secs(1),
            failure_window: Duration::from_secs(60),
        };

        let cb = CircuitBreaker::with_config("test", config);

        // Open the circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Transition to half-open
        std::thread::sleep(Duration::from_millis(1100));
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Record successes to close
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn test_half_open_to_open_on_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            timeout: Duration::from_secs(1),
            failure_window: Duration::from_secs(60),
        };

        let cb = CircuitBreaker::with_config("test", config);

        // Open the circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Transition to half-open
        std::thread::sleep(Duration::from_millis(1100));
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Any failure in half-open goes back to open
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.can_execute());
    }

    #[test]
    fn test_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            timeout: Duration::from_secs(30),
            failure_window: Duration::from_secs(60),
        };

        let cb = CircuitBreaker::with_config("test", config);

        // Open the circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Reset should restore to closed
        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.failure_count.load(Ordering::Acquire), 0);
        assert_eq!(cb.success_count.load(Ordering::Acquire), 0);
        assert!(cb.can_execute());
    }

    #[tokio::test]
    async fn test_execute_success() {
        let cb = CircuitBreaker::new("test");
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = cb
            .execute(|| async move {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Ok::<_, TestError>(42)
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_execute_failure() {
        let cb = CircuitBreaker::new("test");
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = cb
            .execute(|| async move {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(TestError::NetworkError)
            })
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            CircuitBreakerError::ServiceError(_) => {}
            _ => panic!("Expected ServiceError"),
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(cb.failure_count.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn test_execute_circuit_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            timeout: Duration::from_secs(30),
            failure_window: Duration::from_secs(60),
        };

        let cb = CircuitBreaker::with_config("test", config);

        // Open the circuit
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let result = cb
            .execute(|| async move {
                called_clone.store(true, Ordering::SeqCst);
                Ok::<_, TestError>(42)
            })
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            CircuitBreakerError::CircuitOpen(msg) => {
                assert!(msg.contains("Circuit breaker 'test' is open"));
            }
            _ => panic!("Expected CircuitOpen error"),
        }

        // Function should not have been called
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_execute_multiple_failures_opens_circuit() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            timeout: Duration::from_secs(30),
            failure_window: Duration::from_secs(60),
        };

        let cb = CircuitBreaker::with_config("test", config);
        let call_count = Arc::new(AtomicU32::new(0));

        // Execute failures until circuit opens
        for i in 0..3 {
            let counter_clone = call_count.clone();
            let result = cb
                .execute(|| async move {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    Err::<i32, _>(TestError::NetworkError)
                })
                .await;

            assert!(result.is_err());

            if i < 2 {
                assert_eq!(cb.state(), CircuitState::Closed);
            } else {
                assert_eq!(cb.state(), CircuitState::Open);
            }
        }

        assert_eq!(call_count.load(Ordering::SeqCst), 3);

        // Next call should be rejected without execution
        let counter_clone = call_count.clone();
        let result = cb
            .execute(|| async move {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                Ok::<_, TestError>(42)
            })
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            CircuitBreakerError::CircuitOpen(_) => {}
            _ => panic!("Expected CircuitOpen error"),
        }

        // Call count should not have increased
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_execute_recovery_flow() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            timeout: Duration::from_millis(500),
            failure_window: Duration::from_secs(60),
        };

        let cb = Arc::new(CircuitBreaker::with_config("test", config));
        let call_count = Arc::new(AtomicU32::new(0));

        // Step 1: Fail to open the circuit
        for _ in 0..2 {
            let counter_clone = call_count.clone();
            let _ = cb
                .execute(|| async move {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    Err::<i32, _>(TestError::NetworkError)
                })
                .await;
        }

        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(call_count.load(Ordering::SeqCst), 2);

        // Step 2: Wait for timeout to transition to half-open
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Step 3: Succeed to close the circuit
        for _ in 0..2 {
            let counter_clone = call_count.clone();
            let result = cb
                .execute(|| async move {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, TestError>(42)
                })
                .await;

            assert!(result.is_ok());
        }

        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(call_count.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn test_concurrent_executions() {
        let config = CircuitBreakerConfig {
            failure_threshold: 10,
            success_threshold: 3,
            timeout: Duration::from_secs(30),
            failure_window: Duration::from_secs(60),
        };

        let cb = Arc::new(CircuitBreaker::with_config("test", config));
        let success_count = Arc::new(AtomicU32::new(0));

        // Spawn multiple concurrent tasks
        let mut handles = vec![];

        for i in 0..20 {
            let cb_clone = cb.clone();
            let counter_clone = success_count.clone();

            let handle = tokio::spawn(async move {
                cb_clone
                    .execute(|| async move {
                        // Simulate some async work
                        tokio::time::sleep(Duration::from_millis(10)).await;

                        counter_clone.fetch_add(1, Ordering::SeqCst);

                        // Fail every 5th call
                        if i % 5 == 0 {
                            Err::<_, TestError>(TestError::NetworkError)
                        } else {
                            Ok(i)
                        }
                    })
                    .await
            });

            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            let _ = handle.await;
        }

        // Circuit should still be closed (only 4 failures out of 10 threshold)
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(success_count.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn test_circuit_breaker_name() {
        let cb = CircuitBreaker::new("github-api");
        assert_eq!(cb.name, "github-api");

        let cb2 = CircuitBreaker::new("claude-api");
        assert_eq!(cb2.name, "claude-api");
    }
}
