//! Rate limiting middleware for VM control endpoints
//!
//! Implements token bucket algorithm with per-endpoint limits.

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Rate limit configuration for an endpoint
#[derive(Debug, Clone, Copy)]
pub struct RateLimit {
    /// Maximum number of requests per window
    pub limit: u32,
    /// Time window duration
    pub window: Duration,
}

impl RateLimit {
    /// Create a new rate limit
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            limit: requests_per_minute,
            window: Duration::from_secs(60),
        }
    }

    /// Create a per-second rate limit
    pub fn per_second(requests: u32) -> Self {
        Self {
            limit: requests,
            window: Duration::from_secs(1),
        }
    }
}

/// Token bucket for rate limiting
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    capacity: f64,
    refill_rate: f64, // tokens per second
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket
    fn new(limit: u32, window: Duration) -> Self {
        let capacity = limit as f64;
        let refill_rate = capacity / window.as_secs_f64();

        Self {
            tokens: capacity,
            capacity,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();

        let new_tokens = elapsed * self.refill_rate;
        self.tokens = (self.tokens + new_tokens).min(self.capacity);
        self.last_refill = now;
    }

    /// Try to consume a token, returns true if successful
    fn try_consume(&mut self) -> bool {
        self.refill();

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Get remaining tokens
    fn remaining(&mut self) -> u32 {
        self.refill();
        self.tokens.floor() as u32
    }

    /// Get time until next token is available
    fn reset_time(&self) -> Duration {
        if self.tokens >= 1.0 {
            Duration::from_secs(0)
        } else {
            let tokens_needed = 1.0 - self.tokens;
            let seconds_needed = tokens_needed / self.refill_rate;
            Duration::from_secs_f64(seconds_needed)
        }
    }
}

/// Rate limiter state
#[derive(Clone)]
pub struct RateLimiter {
    /// Buckets keyed by (endpoint_pattern, optional_key)
    buckets: Arc<DashMap<String, Arc<Mutex<TokenBucket>>>>,
    /// Rate limits per endpoint pattern
    limits: Arc<DashMap<String, RateLimit>>,
}

impl RateLimiter {
    /// Create a new rate limiter
    pub fn new() -> Self {
        Self {
            buckets: Arc::new(DashMap::new()),
            limits: Arc::new(DashMap::new()),
        }
    }

    /// Configure rate limit for an endpoint pattern
    pub fn configure(&self, pattern: &str, limit: RateLimit) {
        debug!(pattern = %pattern, limit = ?limit, "Configured rate limit");
        self.limits.insert(pattern.to_string(), limit);
    }

    /// Check if request should be rate limited
    pub fn check(&self, endpoint: &str, key: Option<&str>) -> RateLimitResult {
        // Find matching endpoint pattern
        let limit = match self.find_limit(endpoint) {
            Some(limit) => limit,
            None => {
                // No rate limit configured, allow
                return RateLimitResult::Allowed {
                    limit: 0,
                    remaining: 0,
                    reset: Duration::from_secs(0),
                };
            }
        };

        // Build bucket key
        let bucket_key = match key {
            Some(k) => format!("{}:{}", endpoint, k),
            None => endpoint.to_string(),
        };

        // Get or create bucket
        let bucket = self
            .buckets
            .entry(bucket_key)
            .or_insert_with(|| Arc::new(Mutex::new(TokenBucket::new(limit.limit, limit.window))))
            .clone();

        let mut bucket_guard = bucket.lock();

        if bucket_guard.try_consume() {
            RateLimitResult::Allowed {
                limit: limit.limit,
                remaining: bucket_guard.remaining(),
                reset: bucket_guard.reset_time(),
            }
        } else {
            let retry_after = bucket_guard.reset_time();
            RateLimitResult::Limited {
                limit: limit.limit,
                retry_after,
            }
        }
    }

    /// Find rate limit for endpoint (supports wildcards)
    fn find_limit(&self, endpoint: &str) -> Option<RateLimit> {
        // Exact match first
        if let Some(limit) = self.limits.get(endpoint) {
            return Some(*limit.value());
        }

        // Pattern matching (e.g., "/api/v1/vms/:name:*" matches "/api/v1/vms/agent-01:start")
        for entry in self.limits.iter() {
            if Self::matches_pattern(endpoint, entry.key()) {
                return Some(*entry.value());
            }
        }

        None
    }

    /// Check if endpoint matches pattern
    fn matches_pattern(endpoint: &str, pattern: &str) -> bool {
        if pattern.contains(':') {
            // Simple pattern matching for VM actions: /vms/:name:action
            let parts: Vec<&str> = endpoint.split('/').collect();
            let pattern_parts: Vec<&str> = pattern.split('/').collect();

            if parts.len() != pattern_parts.len() {
                return false;
            }

            for (part, pattern_part) in parts.iter().zip(pattern_parts.iter()) {
                // Handle patterns like ":name:start" or ":name:*"
                if pattern_part.starts_with(':') {
                    // Check if pattern has action suffix like ":name:start"
                    if let Some(action_pattern) = pattern_part.split(':').nth(2) {
                        // Pattern has action suffix, check if endpoint matches
                        if let Some(action) = part.split(':').nth(1) {
                            if action_pattern != "*" && action != action_pattern {
                                return false;
                            }
                        } else {
                            // Pattern expects action but endpoint has none
                            return false;
                        }
                    }
                    continue;
                }
                if pattern_part == &"*" {
                    continue;
                }
                if part != pattern_part {
                    return false;
                }
            }

            true
        } else {
            false
        }
    }

    /// Clean up expired buckets
    pub fn cleanup(&self) {
        // Remove buckets that haven't been used recently
        let cutoff = Instant::now() - Duration::from_secs(300); // 5 minutes

        self.buckets.retain(|_, bucket| {
            let guard = bucket.lock();
            guard.last_refill > cutoff
        });
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of rate limit check
#[derive(Debug)]
pub enum RateLimitResult {
    Allowed {
        limit: u32,
        remaining: u32,
        reset: Duration,
    },
    Limited {
        limit: u32,
        retry_after: Duration,
    },
}

/// Rate limit error response
#[derive(Serialize)]
struct RateLimitError {
    error: RateLimitErrorDetail,
}

#[derive(Serialize)]
struct RateLimitErrorDetail {
    code: String,
    message: String,
    retry_after_seconds: u64,
}

/// Axum middleware for rate limiting
pub async fn rate_limit_middleware(
    limiter: Arc<RateLimiter>,
    endpoint_pattern: String,
    extract_key: Option<fn(&Request) -> Option<String>>,
) -> impl Fn(Request, Next) -> futures_util::future::BoxFuture<'static, Response> + Clone {
    move |req: Request, next: Next| {
        let limiter = limiter.clone();
        let endpoint_pattern = endpoint_pattern.clone();
        let key = extract_key.and_then(|f| f(&req));

        Box::pin(async move {
            let result = limiter.check(&endpoint_pattern, key.as_deref());

            match result {
                RateLimitResult::Allowed {
                    limit,
                    remaining,
                    reset,
                } => {
                    let mut response = next.run(req).await;
                    let headers = response.headers_mut();

                    headers.insert(
                        "X-RateLimit-Limit",
                        HeaderValue::from_str(&limit.to_string()).unwrap(),
                    );
                    headers.insert(
                        "X-RateLimit-Remaining",
                        HeaderValue::from_str(&remaining.to_string()).unwrap(),
                    );
                    headers.insert(
                        "X-RateLimit-Reset",
                        HeaderValue::from_str(&reset.as_secs().to_string()).unwrap(),
                    );

                    response
                }
                RateLimitResult::Limited { limit, retry_after } => {
                    warn!(
                        endpoint = %endpoint_pattern,
                        key = ?key,
                        "Rate limit exceeded"
                    );

                    let error = RateLimitError {
                        error: RateLimitErrorDetail {
                            code: "RATE_LIMIT_EXCEEDED".to_string(),
                            message: format!(
                                "Rate limit of {} requests exceeded. Try again in {} seconds.",
                                limit,
                                retry_after.as_secs()
                            ),
                            retry_after_seconds: retry_after.as_secs(),
                        },
                    };

                    let mut headers = HeaderMap::new();
                    headers.insert(
                        "X-RateLimit-Limit",
                        HeaderValue::from_str(&limit.to_string()).unwrap(),
                    );
                    headers.insert("X-RateLimit-Remaining", HeaderValue::from_static("0"));
                    headers.insert(
                        "Retry-After",
                        HeaderValue::from_str(&retry_after.as_secs().to_string()).unwrap(),
                    );

                    (StatusCode::TOO_MANY_REQUESTS, headers, Json(error)).into_response()
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_bucket_new() {
        let bucket = TokenBucket::new(60, Duration::from_secs(60));
        assert_eq!(bucket.capacity, 60.0);
        assert_eq!(bucket.tokens, 60.0);
        assert_eq!(bucket.refill_rate, 1.0); // 60 tokens per 60 seconds = 1/sec
    }

    #[test]
    fn test_token_bucket_consume() {
        let mut bucket = TokenBucket::new(10, Duration::from_secs(10));

        // Should be able to consume all tokens
        for _ in 0..10 {
            assert!(bucket.try_consume());
        }

        // Should fail on 11th attempt
        assert!(!bucket.try_consume());
    }

    #[test]
    fn test_token_bucket_remaining() {
        let mut bucket = TokenBucket::new(10, Duration::from_secs(10));

        assert_eq!(bucket.remaining(), 10);

        bucket.try_consume();
        assert_eq!(bucket.remaining(), 9);

        bucket.try_consume();
        assert_eq!(bucket.remaining(), 8);
    }

    #[test]
    fn test_rate_limiter_configure() {
        let limiter = RateLimiter::new();
        let limit = RateLimit::new(60);

        limiter.configure("/api/v1/vms", limit);

        assert!(limiter.limits.contains_key("/api/v1/vms"));
    }

    #[test]
    fn test_rate_limiter_check_allowed() {
        let limiter = RateLimiter::new();
        limiter.configure("/api/v1/vms", RateLimit::new(10));

        match limiter.check("/api/v1/vms", None) {
            RateLimitResult::Allowed {
                limit,
                remaining,
                reset: _,
            } => {
                assert_eq!(limit, 10);
                assert_eq!(remaining, 9);
            }
            RateLimitResult::Limited { .. } => panic!("Should be allowed"),
        }
    }

    #[test]
    fn test_rate_limiter_check_limited() {
        let limiter = RateLimiter::new();
        limiter.configure("/api/v1/vms", RateLimit::new(2));

        // Consume all tokens
        limiter.check("/api/v1/vms", None);
        limiter.check("/api/v1/vms", None);

        // Should be limited now
        match limiter.check("/api/v1/vms", None) {
            RateLimitResult::Allowed { .. } => panic!("Should be limited"),
            RateLimitResult::Limited { limit, .. } => {
                assert_eq!(limit, 2);
            }
        }
    }

    #[test]
    fn test_rate_limiter_per_key() {
        let limiter = RateLimiter::new();
        limiter.configure("/api/v1/vms/:name:start", RateLimit::new(2));

        // Each key should have its own bucket
        match limiter.check("/api/v1/vms/:name:start", Some("agent-01")) {
            RateLimitResult::Allowed { remaining, .. } => assert_eq!(remaining, 1),
            _ => panic!("Should be allowed"),
        }

        match limiter.check("/api/v1/vms/:name:start", Some("agent-02")) {
            RateLimitResult::Allowed { remaining, .. } => assert_eq!(remaining, 1),
            _ => panic!("Should be allowed"),
        }
    }

    #[test]
    fn test_rate_limiter_no_limit() {
        let limiter = RateLimiter::new();

        // No limit configured, should always allow
        match limiter.check("/api/v1/unknown", None) {
            RateLimitResult::Allowed { limit, .. } => assert_eq!(limit, 0),
            _ => panic!("Should be allowed"),
        }
    }

    #[test]
    fn test_matches_pattern() {
        assert!(RateLimiter::matches_pattern(
            "/api/v1/vms/agent-01:start",
            "/api/v1/vms/:name:start"
        ));

        assert!(RateLimiter::matches_pattern(
            "/api/v1/vms/agent-02:stop",
            "/api/v1/vms/:name:*"
        ));

        assert!(!RateLimiter::matches_pattern(
            "/api/v1/vms/agent-01",
            "/api/v1/vms/:name:start"
        ));
    }

    #[test]
    fn test_rate_limit_new() {
        let limit = RateLimit::new(60);
        assert_eq!(limit.limit, 60);
        assert_eq!(limit.window, Duration::from_secs(60));
    }

    #[test]
    fn test_rate_limit_per_second() {
        let limit = RateLimit::per_second(10);
        assert_eq!(limit.limit, 10);
        assert_eq!(limit.window, Duration::from_secs(1));
    }
}
