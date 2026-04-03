//! OpenTelemetry integration (feature-flagged)
//!
//! Provides distributed tracing export via OTLP when the `otel` feature is enabled.
//!
//! Configuration via environment variables:
//! - OTEL_EXPORTER_OTLP_ENDPOINT: OTLP collector endpoint (e.g., http://localhost:4317)
//! - OTEL_SERVICE_NAME: Service name (default: agentic-management)
//! - OTEL_TRACES_SAMPLER: Sampling strategy (default: always_on)

#[cfg(feature = "otel")]
use opentelemetry::trace::TracerProvider as _;
#[cfg(feature = "otel")]
use opentelemetry_sdk::trace::TracerProvider;

/// OpenTelemetry configuration
#[derive(Debug, Clone)]
pub struct OtelConfig {
    /// OTLP endpoint URL
    pub endpoint: Option<String>,
    /// Service name for traces
    pub service_name: String,
}

impl Default for OtelConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            service_name: "agentic-management".to_string(),
        }
    }
}

impl OtelConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            service_name: std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "agentic-management".to_string()),
        }
    }

    /// Check if OpenTelemetry export is enabled
    pub fn is_enabled(&self) -> bool {
        self.endpoint.is_some()
    }
}

/// OpenTelemetry guard - keeps the tracer provider alive
pub struct OtelGuard {
    #[cfg(feature = "otel")]
    _provider: Option<TracerProvider>,
}

impl OtelGuard {
    /// Create a no-op guard when OTEL is disabled
    pub fn disabled() -> Self {
        Self {
            #[cfg(feature = "otel")]
            _provider: None,
        }
    }
}

/// Initialize OpenTelemetry tracing (only when feature is enabled)
#[cfg(feature = "otel")]
pub fn init_otel(config: &OtelConfig) -> anyhow::Result<OtelGuard> {
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::Resource;

    let endpoint = match &config.endpoint {
        Some(ep) => ep,
        None => {
            tracing::info!("OpenTelemetry disabled (OTEL_EXPORTER_OTLP_ENDPOINT not set)");
            return Ok(OtelGuard::disabled());
        }
    };

    tracing::info!("Initializing OpenTelemetry with endpoint: {}", endpoint);

    // Create OTLP exporter
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    // Create tracer provider with resource attributes
    let resource = Resource::new(vec![
        KeyValue::new("service.name", config.service_name.clone()),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION").to_string()),
    ]);

    let provider = TracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .build();

    // Get tracer from provider
    let _tracer = provider.tracer("agentic-management");

    // Note: The layer needs to be added to the subscriber in mod.rs
    // This function just sets up the provider
    tracing::info!("OpenTelemetry initialized successfully");

    Ok(OtelGuard {
        _provider: Some(provider),
    })
}

/// Initialize OpenTelemetry (no-op when feature is disabled)
#[cfg(not(feature = "otel"))]
pub fn init_otel(_config: &OtelConfig) -> anyhow::Result<OtelGuard> {
    if _config.endpoint.is_some() {
        tracing::warn!(
            "OTEL_EXPORTER_OTLP_ENDPOINT set but 'otel' feature not enabled. \
             Build with --features otel to enable OpenTelemetry export."
        );
    }
    Ok(OtelGuard::disabled())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_otel_config_default() {
        let config = OtelConfig::default();
        assert!(!config.is_enabled());
        assert_eq!(config.service_name, "agentic-management");
    }

    #[test]
    fn test_otel_disabled_without_endpoint() {
        let config = OtelConfig::default();
        let guard = init_otel(&config).unwrap();
        // Should succeed without endpoint
        drop(guard);
    }
}
