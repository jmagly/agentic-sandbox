//! Telemetry module for logging, metrics, and tracing
//!
//! Provides unified telemetry infrastructure with:
//! - Structured logging (JSON/pretty/compact formats)
//! - File output with rotation
//! - Prometheus metrics (builtin or feature-flagged exporter)
//! - Trace ID propagation for distributed tracing
//! - OpenTelemetry export (feature-flagged via `otel`)

mod logging;
mod metrics;
mod otel;
mod trace_id;

pub use logging::{init_logging, LogConfig};
pub use metrics::{Metrics, MetricsConfig};
pub use otel::{init_otel, OtelConfig, OtelGuard};
pub use trace_id::{TraceId, TraceIdLayer, extract_trace_id, generate_trace_id};

use anyhow::Result;
use std::sync::Arc;
use tracing::info;

/// Combined telemetry configuration
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub log: LogConfig,
    pub metrics: MetricsConfig,
    pub otel: OtelConfig,
}

impl TelemetryConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            log: LogConfig::from_env(),
            metrics: MetricsConfig::from_env(),
            otel: OtelConfig::from_env(),
        }
    }
}

/// Initialize all telemetry subsystems
///
/// Call this early in main() before any logging occurs.
/// Returns a guard that must be kept alive for file logging to work.
pub fn init_telemetry(config: &TelemetryConfig) -> Result<TelemetryGuard> {
    // Initialize logging first
    let log_guard = init_logging(&config.log)?;

    // Initialize metrics if enabled
    let metrics = if config.metrics.enabled {
        Some(Arc::new(Metrics::new(&config.metrics)?))
    } else {
        None
    };

    // Initialize OpenTelemetry if configured
    let otel_guard = init_otel(&config.otel)?;

    info!(
        log_format = %config.log.format,
        log_level = %config.log.level,
        metrics_enabled = config.metrics.enabled,
        otel_enabled = config.otel.is_enabled(),
        "Telemetry initialized"
    );

    Ok(TelemetryGuard {
        _log_guard: log_guard,
        _otel_guard: otel_guard,
        metrics,
    })
}

/// Guard that keeps telemetry subsystems alive
pub struct TelemetryGuard {
    _log_guard: logging::LogGuard,
    _otel_guard: OtelGuard,
    pub metrics: Option<Arc<Metrics>>,
}
