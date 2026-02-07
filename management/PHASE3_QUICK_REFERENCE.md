# Phase 3 Quick Reference

Quick reference for using the idempotency, rate limiting, and validation modules.

## Module Locations

```
management/src/http/
├── idempotency.rs    # Idempotency key support
├── rate_limit.rs     # Rate limiting middleware
└── validation.rs     # Input validation
```

## Idempotency

### Basic Usage

```rust
use crate::http::idempotency::{IdempotencyStore, IDEMPOTENCY_KEY_HEADER};

// In handler
async fn my_handler(headers: HeaderMap, state: State<AppState>) -> Result<Response, Error> {
    // Check for cached response
    if let Some(key) = IdempotencyStore::extract_key(&headers) {
        if let Some(cached) = state.idempotency_store.get(&key) {
            return Ok(Response::builder()
                .status(cached.status)
                .header("X-Idempotency-Replay", "true")
                .body(Body::from(cached.body))
                .unwrap());
        }
    }

    // ... perform operation ...

    // Cache response
    if let Some(key) = IdempotencyStore::extract_key(&headers) {
        state.idempotency_store.insert(key, status, body.clone());
    }

    Ok(response)
}
```

### Client Usage

```bash
curl -X POST http://localhost:8122/api/v1/vms/agent-01:start \
  -H "Idempotency-Key: start-agent01-$(date +%Y%m%d-%H%M%S)"
```

## Rate Limiting

### Configuration

```rust
use crate::http::rate_limit::{RateLimit, RateLimiter};

let limiter = RateLimiter::new();

// Configure endpoints
limiter.configure("/api/v1/vms", RateLimit::new(60)); // 60 requests/min
limiter.configure("/api/v1/vms/:name:start", RateLimit::new(30)); // 30 req/min per VM
```

### Check in Handler

```rust
use crate::http::rate_limit::RateLimitResult;

match state.rate_limiter.check("/api/v1/vms/:name:start", Some(&vm_name)) {
    RateLimitResult::Allowed { limit, remaining, reset } => {
        // Add headers to response
        response.headers_mut().insert("X-RateLimit-Limit", limit.into());
        response.headers_mut().insert("X-RateLimit-Remaining", remaining.into());
    }
    RateLimitResult::Limited { limit, retry_after } => {
        return Err(RateLimitError::new(limit, retry_after));
    }
}
```

### Rate Limit Headers

Responses include:
- `X-RateLimit-Limit`: Maximum requests allowed
- `X-RateLimit-Remaining`: Requests remaining in window
- `X-RateLimit-Reset`: Seconds until limit resets
- `Retry-After`: (429 only) Seconds to wait before retry

## Validation

### VM Names

```rust
use crate::http::validation::validate_vm_name;

validate_vm_name("agent-01")?; // OK
validate_vm_name("Agent-01")?; // Error: uppercase not allowed
validate_vm_name("vm-01")?;    // Error: must start with "agent-"
```

**Rules**:
- Pattern: `^agent-[a-z0-9-]+$`
- Max length: 63 characters
- Lowercase only, no special characters except hyphen

### Resources

```rust
use crate::http::validation::validate_resources;

validate_resources(4, 8192, 50)?; // OK: 4 vCPUs, 8GB RAM, 50GB disk
validate_resources(64, 8192, 50)?; // Error: too many CPUs (max 32)
validate_resources(4, 100000, 50)?; // Error: too much memory (max 65536 MB)
```

**Limits**:
- vCPUs: 1-32
- Memory: 1-65536 MB
- Disk: 1-500 GB

### Profiles

```rust
use crate::http::validation::validate_profile;

validate_profile("agentic-dev")?; // OK
validate_profile("basic")?;       // OK
validate_profile("production")?;  // Error: invalid profile
```

**Valid profiles**: `agentic-dev`, `basic`

## Error Handling

### Validation Errors

```rust
use crate::http::validation::{ValidationError, ValidationErrorResponse};

match validate_vm_name(&name) {
    Ok(_) => { /* proceed */ }
    Err(e) => {
        return Err(VmError::ValidationError(e));
    }
}
```

**Error Codes**:
- `INVALID_VM_NAME`
- `VM_NAME_TOO_LONG`
- `TOO_MANY_CPUS`
- `TOO_MUCH_MEMORY`
- `DISK_TOO_LARGE`
- `INVALID_RESOURCE_VALUE`
- `INVALID_PROFILE`

### Rate Limit Errors

**Response**: 429 Too Many Requests

```json
{
  "error": {
    "code": "RATE_LIMIT_EXCEEDED",
    "message": "Rate limit of 30 requests exceeded. Try again in 2 seconds.",
    "retry_after_seconds": 2
  }
}
```

## Maintenance

### Periodic Cleanup

```rust
// Spawn cleanup task (optional, but recommended)
tokio::spawn(async move {
    let mut ticker = tokio::time::interval(Duration::from_secs(300));
    loop {
        ticker.tick().await;
        idempotency_store.cleanup_expired();
        rate_limiter.cleanup();
    }
});
```

### Statistics

```rust
// Get idempotency cache stats
let stats = idempotency_store.stats();
tracing::info!(
    "Idempotency cache size: {} entries",
    stats.total_entries
);
```

## Testing

### Run Phase 3 Tests

```bash
cd management
cargo test idempotency rate_limit validation
```

### Test Idempotency

```bash
# First request
curl -X POST http://localhost:8122/api/v1/vms/agent-01:start \
  -H "Idempotency-Key: test-key-123"

# Duplicate request (returns cached response)
curl -X POST http://localhost:8122/api/v1/vms/agent-01:start \
  -H "Idempotency-Key: test-key-123"
```

### Test Rate Limiting

```bash
# Exceed rate limit
for i in {1..35}; do
  curl -X POST http://localhost:8122/api/v1/vms/agent-01:start
done
# Should get 429 after 30 requests
```

## Integration Checklist

When integrating Phase 3 into a handler:

- [ ] Add `idempotency_store` and `rate_limiter` to `AppState`
- [ ] Add `headers: HeaderMap` parameter to handler
- [ ] Call validation functions for inputs
- [ ] Check rate limits before processing
- [ ] Check idempotency cache before processing
- [ ] Cache successful responses for idempotency
- [ ] Add rate limit headers to responses
- [ ] Handle validation errors appropriately
- [ ] Test all three features together

## Common Patterns

### Full Handler Example

```rust
async fn vm_operation(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Response, VmError> {
    // 1. Validate
    validation::validate_vm_name(&name)?;

    // 2. Rate Limit
    match state.rate_limiter.check("/api/v1/vms/:name:start", Some(&name)) {
        RateLimitResult::Limited { retry_after, .. } => {
            return Err(VmError::RateLimited(retry_after));
        }
        _ => {}
    }

    // 3. Idempotency Check
    if let Some(cached) = handle_idempotent_response(&headers, &state.idempotency_store) {
        return Ok(cached);
    }

    // 4. Perform Operation
    let result = perform_operation(&name).await?;

    // 5. Cache Response
    let body = serde_json::to_vec(&result)?;
    cache_response(&headers, &state.idempotency_store, StatusCode::OK, &body);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap())
}
```

## Configuration

### Rate Limits (from spec)

```rust
// In HttpServer::new()
rate_limiter.configure("/api/v1/vms", RateLimit::new(60));           // List VMs
rate_limiter.configure("/api/v1/vms/:name", RateLimit::new(120));    // Get VM
rate_limiter.configure("/api/v1/vms/create", RateLimit::new(10));    // Create
rate_limiter.configure("/api/v1/vms/:name:start", RateLimit::new(30)); // Start
rate_limiter.configure("/api/v1/vms/:name:stop", RateLimit::new(30));  // Stop
rate_limiter.configure("/api/v1/vms/:name:restart", RateLimit::new(30)); // Restart
rate_limiter.configure("/api/v1/vms/:name:destroy", RateLimit::new(30)); // Destroy
rate_limiter.configure("/api/v1/vms/:name/delete", RateLimit::new(10)); // Delete
```

### Idempotency TTL

Default: 24 hours (configurable in code via `CACHE_TTL` constant)

## Performance Tips

1. **Idempotency**: Keys are hashed, lookups are O(1)
2. **Rate Limiting**: Token buckets refill lazily, minimal overhead
3. **Validation**: Regex compilation cached via `OnceLock`
4. **Cleanup**: Run every 5 minutes, not every request
5. **Memory**: ~500 bytes per cached idempotency response, ~200 bytes per rate limit bucket

## Troubleshooting

### Idempotency Not Working
- Check header name is exactly `idempotency-key` (lowercase)
- Verify key is under 255 characters
- Check if response was already cached (24h TTL)

### Rate Limits Too Strict
- Adjust limits in `configure()` calls
- Check if per-key isolation is needed
- Verify pattern matching is correct

### Validation Too Strict
- Review allowed patterns in `validation.rs`
- Check resource limits match your infrastructure
- Ensure profile names are in allowed list

## References

- Implementation: `VM_CONTROL_API_PHASE_3_IMPLEMENTATION.md`
- Integration Example: `examples/phase3_integration.rs`
- API Spec: `docs/api/vm-control.md`
