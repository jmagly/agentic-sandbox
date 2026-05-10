//! `agentic-sandbox/idempotency` extension. Filled in by #213.
//!
//! Wraps the v2 `IdempotencyCache` (Wave 2 W2.2 / #206) and applies
//! `Idempotency-Key` header + JCS-canonical-payload deduplication to
//! `message/send` and `tasks/cancel`.

/// Idempotency-extension marker.
pub struct IdempotencyExtension;
