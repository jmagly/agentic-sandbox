//! Structured logging with format selection and file rotation
//!
//! Configuration via environment variables:
//! - LOG_LEVEL: trace, debug, info, warn, error (default: info)
//! - LOG_FORMAT: pretty, json, compact (default: pretty)
//! - LOG_FILE: Optional file path for log output
//! - LOG_FILE_ROTATION: hourly, daily, never (default: daily)
//! - LOG_FILE_RETENTION_DAYS: Days to retain logs (default: 7)

use anyhow::Result;
use std::env;
use std::path::PathBuf;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

/// Log output format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Human-readable colored output (default)
    #[default]
    Pretty,
    /// Machine-readable JSON
    Json,
    /// Compact single-line format
    Compact,
}

impl std::fmt::Display for LogFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogFormat::Pretty => write!(f, "pretty"),
            LogFormat::Json => write!(f, "json"),
            LogFormat::Compact => write!(f, "compact"),
        }
    }
}

impl std::str::FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pretty" => Ok(LogFormat::Pretty),
            "json" => Ok(LogFormat::Json),
            "compact" => Ok(LogFormat::Compact),
            _ => Err(format!("unknown log format: {}", s)),
        }
    }
}

/// File rotation policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileRotation {
    /// Rotate hourly
    Hourly,
    /// Rotate daily (default)
    #[default]
    Daily,
    /// Never rotate
    Never,
}

impl std::str::FromStr for FileRotation {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hourly" => Ok(FileRotation::Hourly),
            "daily" => Ok(FileRotation::Daily),
            "never" => Ok(FileRotation::Never),
            _ => Err(format!("unknown rotation policy: {}", s)),
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Log level
    pub level: String,
    /// Output format
    pub format: LogFormat,
    /// Optional file path for logging
    pub file: Option<PathBuf>,
    /// File rotation policy
    pub rotation: FileRotation,
    /// Days to retain rotated log files
    pub retention_days: u32,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: LogFormat::Pretty,
            file: None,
            rotation: FileRotation::Daily,
            retention_days: 7,
        }
    }
}

impl LogConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            level: env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
            format: env::var("LOG_FORMAT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            file: env::var("LOG_FILE").ok().map(PathBuf::from),
            rotation: env::var("LOG_FILE_ROTATION")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            retention_days: env::var("LOG_FILE_RETENTION_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(7),
        }
    }
}

/// Guard that keeps the logging worker thread alive
pub struct LogGuard {
    _guards: Vec<WorkerGuard>,
}

/// Initialize the logging subsystem
pub fn init_logging(config: &LogConfig) -> Result<LogGuard> {
    let mut guards = Vec::new();

    // Build environment filter
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            EnvFilter::new(&config.level)
                .add_directive("agentic_management=info".parse().unwrap())
                .add_directive("tonic=info".parse().unwrap())
                .add_directive("tower=warn".parse().unwrap())
                .add_directive("hyper=warn".parse().unwrap())
        });

    // Create stdout layer based on format
    let (stdout_layer, file_layer): (Box<dyn Layer<_> + Send + Sync>, Option<Box<dyn Layer<_> + Send + Sync>>) =
        match config.format {
            LogFormat::Pretty => {
                let layer = fmt::layer()
                    .with_ansi(true)
                    .with_target(true)
                    .with_thread_ids(false)
                    .with_span_events(FmtSpan::NONE);
                (Box::new(layer), None)
            }
            LogFormat::Json => {
                let layer = fmt::layer()
                    .json()
                    .with_target(true)
                    .with_thread_ids(true)
                    .with_span_events(FmtSpan::CLOSE)
                    .with_current_span(true);
                (Box::new(layer), None)
            }
            LogFormat::Compact => {
                let layer = fmt::layer()
                    .compact()
                    .with_ansi(true)
                    .with_target(false);
                (Box::new(layer), None)
            }
        };

    // Create file layer if configured
    let file_layer = if let Some(ref file_path) = config.file {
        let dir = file_path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let filename = file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("agentic-mgmt.log");

        let rotation = match config.rotation {
            FileRotation::Hourly => Rotation::HOURLY,
            FileRotation::Daily => Rotation::DAILY,
            FileRotation::Never => Rotation::NEVER,
        };

        let file_appender = RollingFileAppender::new(rotation, dir, filename);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        guards.push(guard);

        // File layer always uses JSON for machine parsing
        let layer = fmt::layer()
            .json()
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(true)
            .with_writer(non_blocking);
        Some(Box::new(layer) as Box<dyn Layer<_> + Send + Sync>)
    } else {
        file_layer
    };

    // Build and initialize subscriber
    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer);

    if let Some(file_layer) = file_layer {
        subscriber.with(file_layer).init();
    } else {
        subscriber.init();
    }

    Ok(LogGuard { _guards: guards })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_format_parse() {
        assert_eq!("pretty".parse::<LogFormat>().unwrap(), LogFormat::Pretty);
        assert_eq!("json".parse::<LogFormat>().unwrap(), LogFormat::Json);
        assert_eq!("compact".parse::<LogFormat>().unwrap(), LogFormat::Compact);
        assert!("invalid".parse::<LogFormat>().is_err());
    }

    #[test]
    fn test_file_rotation_parse() {
        assert_eq!("hourly".parse::<FileRotation>().unwrap(), FileRotation::Hourly);
        assert_eq!("daily".parse::<FileRotation>().unwrap(), FileRotation::Daily);
        assert_eq!("never".parse::<FileRotation>().unwrap(), FileRotation::Never);
    }

    #[test]
    fn test_config_from_env() {
        // Default config
        let config = LogConfig::from_env();
        assert_eq!(config.retention_days, 7);
    }
}
