//! Trace ID generation and propagation
//!
//! Provides trace correlation across service boundaries:
//! - Generate unique trace IDs using UUIDv7 (time-ordered)
//! - Propagate via gRPC metadata (x-trace-id header)
//! - Include in all log entries via tracing span
//!
//! # UUIDv7 Benefits
//!
//! UUIDv7 embeds a Unix timestamp (milliseconds) in the first 48 bits,
//! providing:
//! - **Time-ordering**: IDs sort chronologically by creation time
//! - **Traceability**: Extract creation timestamp from any trace ID
//! - **Deterministic ordering**: Consistent ordering across distributed systems

use std::fmt;
use std::str::FromStr;
use tonic::metadata::MetadataValue;
use tracing::{Span, Level};
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::Layer;
use uuid::Uuid;

/// Trace ID type - a time-ordered unique identifier for request tracing
///
/// Uses UUIDv7 format (RFC 9562) which embeds millisecond-precision timestamps
/// enabling time-based ordering and efficient temporal queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceId(Uuid);

impl TraceId {
    /// Create a new UUIDv7 trace ID with current timestamp
    ///
    /// The trace ID embeds the current Unix timestamp (milliseconds) in the
    /// first 48 bits, making IDs naturally sortable by creation time.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Create from existing UUID
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Get the underlying UUID
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    /// Extract the timestamp from a UUIDv7 trace ID
    ///
    /// Returns the Unix timestamp in milliseconds, or None if not a v7 UUID.
    pub fn timestamp_millis(&self) -> Option<u64> {
        // Check version field (bits 48-51 should be 0111 = 7)
        let bytes = self.0.as_bytes();
        let version = (bytes[6] >> 4) & 0x0F;
        if version != 7 {
            return None;
        }

        // Extract 48-bit timestamp from first 6 bytes
        let millis = ((bytes[0] as u64) << 40)
            | ((bytes[1] as u64) << 32)
            | ((bytes[2] as u64) << 24)
            | ((bytes[3] as u64) << 16)
            | ((bytes[4] as u64) << 8)
            | (bytes[5] as u64);

        Some(millis)
    }

    /// Check if this is a valid UUIDv7
    pub fn is_v7(&self) -> bool {
        let bytes = self.0.as_bytes();
        let version = (bytes[6] >> 4) & 0x0F;
        version == 7
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use simple format for logging (no hyphens)
        write!(f, "{}", self.0.simple())
    }
}

impl FromStr for TraceId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Header name for trace ID propagation
pub const TRACE_ID_HEADER: &str = "x-trace-id";

/// Generate a new trace ID
pub fn generate_trace_id() -> TraceId {
    TraceId::new()
}

/// Extract trace ID from gRPC metadata
pub fn extract_trace_id<T>(request: &tonic::Request<T>) -> Option<TraceId> {
    request
        .metadata()
        .get(TRACE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

/// Add trace ID to gRPC request metadata
pub fn inject_trace_id<T>(request: &mut tonic::Request<T>, trace_id: &TraceId) {
    if let Ok(value) = MetadataValue::try_from(trace_id.to_string().as_str()) {
        request.metadata_mut().insert(TRACE_ID_HEADER, value);
    }
}

/// Create a tracing span with trace ID
pub fn trace_span(name: &'static str, trace_id: &TraceId) -> Span {
    tracing::span!(
        Level::INFO,
        "request",
        trace_id = %trace_id,
        otel.name = name
    )
}

/// Tracing layer that adds trace IDs to spans
pub struct TraceIdLayer;

impl<S> Layer<S> for TraceIdLayer
where
    S: tracing::Subscriber,
{
    fn on_new_span(
        &self,
        _attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: LayerContext<'_, S>,
    ) {
        // The trace_id field is already recorded in the span attributes
        // This layer can be extended to do additional processing
    }
}

/// Tower layer for HTTP/gRPC trace ID injection
#[derive(Clone)]
pub struct TraceIdService<S> {
    inner: S,
}

impl<S> TraceIdService<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

/// Tower layer factory
#[derive(Clone, Default)]
pub struct TraceIdLayerFactory;

impl<S> tower::Layer<S> for TraceIdLayerFactory {
    type Service = TraceIdService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TraceIdService::new(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_id_generation() {
        let id1 = TraceId::new();
        let id2 = TraceId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_trace_id_is_v7() {
        let id = TraceId::new();
        assert!(id.is_v7(), "Generated trace ID should be UUIDv7");
    }

    #[test]
    fn test_trace_id_roundtrip() {
        let id = TraceId::new();
        let s = id.to_string();
        let parsed: TraceId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_trace_id_display() {
        let id = TraceId::new();
        let s = id.to_string();
        // Simple format has no hyphens
        assert!(!s.contains('-'));
        assert_eq!(s.len(), 32);
    }

    #[test]
    fn test_trace_id_timestamp_extraction() {
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let id = TraceId::new();

        let after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let timestamp = id.timestamp_millis().expect("Should extract timestamp from v7 UUID");
        assert!(timestamp >= before, "Timestamp should be >= creation time");
        assert!(timestamp <= after, "Timestamp should be <= current time");
    }

    #[test]
    fn test_trace_id_ordering() {
        // UUIDv7 IDs should be lexicographically ordered by creation time
        let id1 = TraceId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let id2 = TraceId::new();

        // When displayed as simple hex, id2 should be greater than id1
        assert!(id2.to_string() > id1.to_string(), "Later trace ID should sort after earlier one");
    }
}
